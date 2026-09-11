//! Explicit Tauri commands for hash-bound content-module lifecycle review.

use lorepia_shell_api as shell;
use tauri::State;

use crate::{
    error::{CommandError, CommandResult},
    state::AppState,
};

#[tauri::command]
pub async fn review_content_module_activation_page(
    state: State<'_, AppState>,
    request: shell::ReviewContentModuleActivationPageInput,
) -> CommandResult<shell::ContentModuleActivationReviewPageDto> {
    let shell = state.shell()?;
    run_module_work(move || {
        shell
            .review_content_module_activation_page(request)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn resolve_content_module_activation_summary(
    state: State<'_, AppState>,
    request: shell::ResolveContentModuleActivationInput,
) -> CommandResult<shell::ContentModuleActivationPlanSummaryDto> {
    let shell = state.shell()?;
    run_module_work(move || {
        shell
            .resolve_content_module_activation_summary(request)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn activate_content_module_summary(
    state: State<'_, AppState>,
    request: shell::ActivateContentModuleInput,
) -> CommandResult<shell::ContentModuleActivationReceiptSummaryDto> {
    let shell = state.shell()?;
    run_module_work(move || {
        shell
            .activate_content_module_summary(request)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn list_content_module_lifecycle_candidates(
    state: State<'_, AppState>,
    request: shell::ListContentModuleLifecycleCandidatesInput,
) -> CommandResult<shell::ContentModuleLifecycleCandidatesDto> {
    let shell = state.shell()?;
    run_module_read(move || execute_list_content_module_lifecycle_candidates(&shell, request)).await
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
pub async fn list_content_module_lifecycle_bindings(
    state: State<'_, AppState>,
    request: shell::ListContentModuleLifecycleBindingsInput,
) -> CommandResult<shell::ContentModuleLifecycleBindingsDto> {
    let shell = state.shell()?;
    run_module_read(move || {
        shell
            .list_content_module_lifecycle_bindings(request)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn review_content_module_activation(
    state: State<'_, AppState>,
    request: shell::ReviewContentModuleActivationInput,
) -> CommandResult<shell::ContentModuleActivationReviewDto> {
    let shell = state.shell()?;
    run_module_work(move || {
        shell
            .review_content_module_activation(request)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn resolve_content_module_activation(
    state: State<'_, AppState>,
    request: shell::ResolveContentModuleActivationInput,
) -> CommandResult<shell::ContentModuleActivationPlanDto> {
    let shell = state.shell()?;
    run_module_work(move || {
        shell
            .resolve_content_module_activation(request)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn activate_content_module(
    state: State<'_, AppState>,
    request: shell::ActivateContentModuleInput,
) -> CommandResult<shell::ContentModuleActivationReceiptDto> {
    let shell = state.shell()?;
    run_module_work(move || {
        shell
            .activate_content_module(request)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn review_content_module_deactivation(
    state: State<'_, AppState>,
    request: shell::ReviewContentModuleDeactivationInput,
) -> CommandResult<shell::ContentModuleDeactivationReviewDto> {
    let shell = state.shell()?;
    run_module_work(move || {
        shell
            .review_content_module_deactivation(request)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn deactivate_content_module(
    state: State<'_, AppState>,
    request: shell::DeactivateContentModuleInput,
) -> CommandResult<shell::ContentModuleDeactivationReceiptDto> {
    let shell = state.shell()?;
    run_module_work(move || {
        shell
            .deactivate_content_module(request)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn review_content_module_rollback(
    state: State<'_, AppState>,
    request: shell::ReviewContentModuleRollbackInput,
) -> CommandResult<shell::ContentModuleRollbackReviewDto> {
    let shell = state.shell()?;
    run_module_work(move || {
        shell
            .review_content_module_rollback(request)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn resolve_content_module_rollback(
    state: State<'_, AppState>,
    request: shell::ResolveContentModuleRollbackInput,
) -> CommandResult<shell::ContentModuleRollbackPlanDto> {
    let shell = state.shell()?;
    run_module_work(move || {
        shell
            .resolve_content_module_rollback(request)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn apply_content_module_rollback(
    state: State<'_, AppState>,
    request: shell::ApplyContentModuleRollbackInput,
) -> CommandResult<shell::ContentModuleActivationReceiptDto> {
    let shell = state.shell()?;
    run_module_work(move || {
        shell
            .apply_content_module_rollback(request)
            .map_err(CommandError::from)
    })
    .await
}

// Hashing large module packages must never block the native event loop. Keep
// worker admission bounded and hold the permit until the blocking work ends,
// even when its awaiting command is cancelled.
static MODULE_WORKERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
static MODULE_READS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(8);

// Opening a room legitimately requests its profile and effects together.
// Queue only a bounded number of reads instead of discarding that room state.
pub(crate) async fn run_module_read<T: Send + 'static>(
    work: impl FnOnce() -> CommandResult<T> + Send + 'static,
) -> CommandResult<T> {
    let admission = MODULE_READS
        .try_acquire()
        .map_err(|_| CommandError::busy())?;
    let worker = MODULE_WORKERS
        .acquire()
        .await
        .map_err(|_| CommandError::internal())?;
    tauri::async_runtime::spawn_blocking(move || {
        let (_admission, _worker) = (admission, worker);
        work()
    })
    .await
    .map_err(|_| CommandError::internal())?
}

pub(crate) async fn run_module_work<T: Send + 'static>(
    work: impl FnOnce() -> CommandResult<T> + Send + 'static,
) -> CommandResult<T> {
    let permit = MODULE_WORKERS
        .try_acquire()
        .map_err(|_| CommandError::busy())?;
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|_| CommandError::internal())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paged_routes_keep_exact_typed_requests_and_receipts() {
        fn route<'a, I, O, F: std::future::Future<Output = CommandResult<O>> + Send>(
            _: fn(State<'a, AppState>, I) -> F,
        ) {
        }
        route::<crate::contract::CharacterRenderProfileRequest, shell::CharacterRenderProfileDto, _>(
            crate::commands::get_character_render_profile,
        );
        route::<shell::ListInteractionEffectHistoryInput, shell::InteractionEffectHistoryPageDto, _>(
            crate::orchestration_commands::list_interaction_effect_history,
        );
        route::<
            shell::ListRecentReopenInteractionEffectsInput,
            shell::InteractionReopenSnapshotDto,
            _,
        >(crate::orchestration_commands::list_reopen_interaction_effects);
        route::<
            shell::ListCompletedContentPackageExportsInput,
            Vec<shell::ContentSourceExportDescriptorDto>,
            _,
        >(crate::package_commands::list_completed_content_package_exports);
        route::<
            shell::ReviewContentModuleActivationPageInput,
            shell::ContentModuleActivationReviewPageDto,
            _,
        >(review_content_module_activation_page);
        route::<
            shell::ResolveContentModuleActivationInput,
            shell::ContentModuleActivationPlanSummaryDto,
            _,
        >(resolve_content_module_activation_summary);
        route::<
            shell::ActivateContentModuleInput,
            shell::ContentModuleActivationReceiptSummaryDto,
            _,
        >(activate_content_module_summary);
        let request = serde_json::json!({
            "activation": {
                "runtime_target": {"conversation_id":"conversation", "branch_id":"branch"},
                "expected_binding_revision": null,
                "binding": {"id":"binding", "module_id":"module", "scope":"branch", "target_id":"branch",
                    "conversation_id":"conversation", "priority":0, "resolution_mode":"pinned", "pinned_revision_id":"revision",
                    "package_import_approval_id":null, "variable_overrides":{"values":[]}}
            }, "offset":64, "expected_review_sha256":"a".repeat(64)
        });
        let decoded: shell::ReviewContentModuleActivationPageInput =
            serde_json::from_value(request.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), request);
        let mut unknown = request;
        unknown["host_path"] = serde_json::json!("forbidden");
        assert!(
            serde_json::from_value::<shell::ReviewContentModuleActivationPageInput>(unknown)
                .is_err()
        );
    }

    #[test]
    fn module_hashing_leaves_the_calling_thread_and_has_bounded_admission() {
        let caller = std::thread::current().id();
        let worker =
            tauri::async_runtime::block_on(run_module_work(|| Ok(std::thread::current().id())))
                .expect("worker result");
        assert_ne!(caller, worker);
        let permits = MODULE_WORKERS.try_acquire_many(2).expect("idle workers");
        let error = tauri::async_runtime::block_on(run_module_work(|| Ok(())))
            .expect_err("excess work must not create another worker");
        assert_eq!(error.code, "busy");
        drop(permits);
        assert!(tauri::async_runtime::block_on(run_module_work(|| Ok(()))).is_ok());

        tauri::async_runtime::block_on(async {
            let workers = MODULE_WORKERS.try_acquire_many(2).unwrap();
            let mut read = Box::pin(run_module_read(|| Ok(std::thread::current().id())));
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(10), &mut read)
                    .await
                    .is_err()
            );
            assert_eq!(MODULE_READS.available_permits(), 7);
            drop(workers);
            assert_ne!(read.await.unwrap(), caller);
            assert_eq!(MODULE_READS.available_permits(), 8);

            let reads = MODULE_READS.try_acquire_many(8).unwrap();
            assert_eq!(run_module_read(|| Ok(())).await.unwrap_err().code, "busy");
            drop(reads);

            let workers = MODULE_WORKERS.try_acquire_many(2).unwrap();
            let mut cancelled = Box::pin(run_module_read(|| Ok(())));
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(10), &mut cancelled)
                    .await
                    .is_err()
            );
            drop(cancelled);
            assert_eq!(MODULE_READS.available_permits(), 8);
            drop(workers);
        });
    }
}
