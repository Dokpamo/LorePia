//! Native-only content-package picker, source export, and durable lifecycle commands.
//!
//! A selected host path exists only inside `pick_content_package_import`.
//! Core immediately snapshots, inspects, and promotes the exact bytes before
//! this command discards the platform-owned staging file on every outcome.
//! Export resolves only a verified Rust-owned CAS source and passes its private
//! path directly to the scoped native save service. The webview receives only
//! an exact post-delivery hash, size, kind, source identifier, and file name.

use lorepia_shell_api as shell;
use tauri::{AppHandle, State};
use tauri_plugin_lorepia_platform::{LorepiaPlatformExt, PlatformError, PlatformErrorCode};

use crate::{
    error::{CommandError, CommandResult},
    state::AppState,
};

#[tauri::command]
pub async fn pick_content_package_import(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Option<shell::ContentPackageInspectionReviewDto>> {
    let shell_api = state.shell()?;
    let Some(staged) = app.lorepia_platform().pick_import().await? else {
        return Ok(None);
    };
    let inspection = execute_inspect_content_package_import(
        &shell_api,
        &shell::StagedImportFile::new(staged.path()),
    );
    let cleanup = app
        .lorepia_platform()
        .discard_staged_import(&staged)
        .await
        .map_err(CommandError::from);

    match (inspection, cleanup) {
        (Ok(review), Ok(())) => Ok(Some(review)),
        (Ok(review), Err(cleanup_error)) => {
            let _ =
                shell_api.discard_content_package_import(shell::DiscardContentPackageImportInput {
                    import_id: review.import_id,
                    expected_revision: review.revision,
                    expected_review_sha256: review.review_sha256,
                    expected_import_plan_sha256: None,
                    expected_capability_review_sha256: review.capability_review_sha256,
                });
            Err(cleanup_error)
        }
        (Err(error), Ok(())) => Err(error),
        (Err(_), Err(cleanup_error)) => Err(cleanup_error),
    }
}

pub(crate) fn execute_inspect_content_package_import(
    shell_api: &shell::ShellApi,
    staged_file: &shell::StagedImportFile,
) -> CommandResult<shell::ContentPackageInspectionReviewDto> {
    shell_api
        .inspect_content_package_import(staged_file)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_completed_content_package_exports(
    state: State<'_, AppState>,
    request: shell::ListCompletedContentPackageExportsInput,
) -> CommandResult<Vec<shell::ContentSourceExportDescriptorDto>> {
    state
        .shell()?
        .list_completed_content_package_exports(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn export_content_source(
    app: AppHandle,
    state: State<'_, AppState>,
    request: shell::ContentSourceExportInput,
) -> CommandResult<Option<shell::ContentSourceExportReceiptDto>> {
    let prepared = state.shell()?.prepare_content_source_export(request)?;
    let descriptor = prepared.descriptor();
    let Some(delivered) = app
        .lorepia_platform()
        .save_content_source(
            prepared.source_path(),
            &descriptor.suggested_file_name,
            descriptor.size_bytes,
            &descriptor.sha256,
        )
        .await?
    else {
        return Ok(None);
    };

    project_content_source_export_receipt(
        descriptor,
        delivered.display_name(),
        delivered.size_bytes(),
        delivered.sha256(),
    )
    .map(Some)
}

/// Fails closed unless native delivery evidence matches the prepared source.
pub(crate) fn project_content_source_export_receipt(
    descriptor: &shell::ContentSourceExportDescriptorDto,
    delivered_file_name: &str,
    delivered_size_bytes: u64,
    delivered_sha256: &str,
) -> CommandResult<shell::ContentSourceExportReceiptDto> {
    if delivered_size_bytes != descriptor.size_bytes
        || delivered_sha256 != descriptor.sha256.as_str()
    {
        return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable).into());
    }

    shell::ContentSourceExportReceiptDto::from_delivered_file_name(
        descriptor,
        delivered_file_name.to_owned(),
    )
    .map_err(CommandError::from)
}

#[tauri::command]
pub fn reopen_content_package_import(
    state: State<'_, AppState>,
    request: shell::ReopenContentPackageImportInput,
) -> CommandResult<shell::ContentPackageWorkspaceDto> {
    state
        .shell()?
        .reopen_content_package_import(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_pending_content_package_imports(
    state: State<'_, AppState>,
    request: shell::ListPendingContentPackageImportsInput,
) -> CommandResult<Vec<shell::ContentPackageImportReviewDto>> {
    state
        .shell()?
        .list_pending_content_package_imports(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn select_content_package_import(
    state: State<'_, AppState>,
    request: shell::SelectContentPackageImportInput,
) -> CommandResult<shell::SelectContentPackageImportReceiptDto> {
    execute_select_content_package_import(&state.shell()?, request)
}

pub(crate) fn execute_select_content_package_import(
    shell_api: &shell::ShellApi,
    request: shell::SelectContentPackageImportInput,
) -> CommandResult<shell::SelectContentPackageImportReceiptDto> {
    shell_api
        .select_content_package_import(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn approve_content_package_import(
    state: State<'_, AppState>,
    request: shell::ApproveContentPackageImportInput,
) -> CommandResult<shell::ApproveContentPackageImportReceiptDto> {
    state
        .shell()?
        .approve_content_package_import(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn commit_content_package_import(
    state: State<'_, AppState>,
    request: shell::CommitContentPackageImportInput,
) -> CommandResult<shell::CommitContentPackageImportReceiptDto> {
    state
        .shell()?
        .commit_content_package_import(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn discard_content_package_import(
    state: State<'_, AppState>,
    request: shell::DiscardContentPackageImportInput,
) -> CommandResult<shell::ContentPackageImportSummaryDto> {
    state
        .shell()?
        .discard_content_package_import(request)
        .map_err(CommandError::from)
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Write, path::Path};

    use lorepia_shell_api as shell;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tempfile::{NamedTempFile, TempDir, tempdir};
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use super::{execute_inspect_content_package_import, execute_select_content_package_import};
    use crate::{
        commands::execute_resolve_asset_delivery,
        module_lifecycle_commands::execute_list_content_module_lifecycle_candidates,
        orchestration_commands::execute_list_content_modules,
    };

    fn write_hostile_package(path: &Path) -> String {
        let module = serde_json::to_vec(&json!({
            "id": "tauri.quarantined-module",
            "name": "Tauri quarantined executable module",
            "version": "1.0.0",
            "schema_version": 1,
            "prompt_fragments": [],
            "knowledge_book_ids": [],
            "control_specs": [],
            "transform_set_ids": [],
            "interaction_rule_set_ids": [],
            "asset_ids": [],
            "required_capabilities": [],
            "script": "fetch('https://invalid.example/tauri-canary')",
            "html": "<script>window.__LOREPIA_TAURI_CANARY__ = true</script>"
        }))
        .expect("encode hostile module");
        let module_sha256 = format!("{:x}", Sha256::digest(&module));
        let manifest = serde_json::to_vec(&json!({
            "format": "lorepia_content_package",
            "format_version": 1,
            "package_id": "dev.lorepia.tauri-quarantined-module-test",
            "name": "Tauri quarantined module fixture",
            "version": "1.0.0",
            "author": "LorePia tests",
            "license": "MIT",
            "redistribution_allowed": true,
            "required_app_version": "0.1.0",
            "required_capabilities": ["content_modules"],
            "dependencies": [],
            "conflicts": [],
            "content_hashes": {"modules/hostile.json": module_sha256.as_str()},
            "content_types": {"modules/hostile.json": "application/json"},
            "components": [{
                "id": "hostile-module",
                "path": "modules/hostile.json",
                "kind": "content_module"
            }],
            "signature": null
        }))
        .expect("encode hostile package manifest");
        let file = File::create(path).expect("create hostile package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        archive
            .start_file("manifest.json", options)
            .expect("start manifest");
        archive.write_all(&manifest).expect("write manifest");
        archive
            .start_file("modules/hostile.json", options)
            .expect("start hostile module");
        archive.write_all(&module).expect("write hostile module");
        archive.finish().expect("finish hostile package");
        module_sha256
    }

    struct RuntimeFixture {
        _root: TempDir,
        shell_api: shell::ShellApi,
        conversation_id: String,
        branch_id: String,
    }

    fn runtime_fixture() -> RuntimeFixture {
        let root = tempdir().expect("temporary Tauri package data root");
        let shell_api = shell::ShellApi::open_data_root(root.path()).expect("open fixture Shell");
        let mut source = NamedTempFile::new().expect("create synthetic character source");
        source
            .write_all(
                br#"{"spec":"chara_card_v3","data":{"name":"Tauri Package","description":"Synthetic hostile-package handler fixture"}}"#,
            )
            .expect("write synthetic character source");
        let inspection = shell_api
            .inspect_import(&shell::StagedImportFile::new(source.path()))
            .expect("inspect synthetic character");
        let character = shell_api
            .commit_import(&inspection.inspection_id)
            .expect("commit synthetic character");
        let conversation = shell_api
            .create_conversation(shell::CreateConversationInput {
                character_id: character.id,
                title: "Tauri hostile package runtime".to_owned(),
                mode: shell::ConversationModeDto::Chat,
                greeting: None,
            })
            .expect("create runtime conversation");
        let state = shell_api
            .get_conversation_state(&conversation.id)
            .expect("load runtime target");
        RuntimeFixture {
            _root: root,
            shell_api,
            conversation_id: conversation.id,
            branch_id: state.active_branch_id,
        }
    }

    #[test]
    fn hostile_package_handler_quarantines_without_module_candidate_or_asset_authority() {
        let fixture = runtime_fixture();
        let package_root = tempdir().expect("temporary hostile package root");
        let package_path = package_root.path().join("hostile-package.zip");
        let module_sha256 = write_hostile_package(&package_path);
        let inspection = execute_inspect_content_package_import(
            &fixture.shell_api,
            &shell::StagedImportFile::new(&package_path),
        )
        .expect("handler inspects hostile package as inert data");
        let component = inspection
            .components
            .iter()
            .find(|component| component.id == "hostile-module")
            .expect("hostile component review");
        assert_eq!(
            component.disposition,
            shell::ContentPackageComponentDispositionDto::Quarantined
        );
        let selection = execute_select_content_package_import(
            &fixture.shell_api,
            shell::SelectContentPackageImportInput {
                import_id: inspection.import_id,
                expected_revision: inspection.revision,
                expected_package_plan_hash: inspection.package_plan_hash,
                expected_review_sha256: inspection.review_sha256,
                expected_capability_review_sha256: inspection.capability_review_sha256,
                selected_component_ids: vec!["hostile-module".to_owned()],
            },
        )
        .expect_err("handler must reject quarantined component selection");
        assert_eq!(selection.code, "unsafe_archive");

        let modules = execute_list_content_modules(&fixture.shell_api)
            .expect("handler lists runtime modules");
        assert!(modules.is_empty());
        let candidates = execute_list_content_module_lifecycle_candidates(
            &fixture.shell_api,
            shell::test_support::synthetic_content_module_lifecycle_candidates_input(
                &fixture.conversation_id,
                &fixture.branch_id,
            ),
        )
        .expect("handler lists module candidates");
        assert!(candidates.items.is_empty());
        let asset = execute_resolve_asset_delivery(
            &fixture.shell_api,
            shell::ResolveAssetDeliveryInput {
                selector: shell::AssetDeliverySelector::Sha256 {
                    sha256: module_sha256,
                },
            },
        )
        .expect_err("quarantined module bytes must not gain asset authority");
        assert_eq!(asset.code, "not_found");
    }
}
