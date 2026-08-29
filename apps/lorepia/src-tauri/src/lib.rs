mod asset_protocol;
mod channels;
mod chat_stream_registry;
mod commands;
pub mod contract;
mod credential_operations;
mod error;
mod module_lifecycle_commands;
mod orchestration_commands;
mod package_commands;
mod persona_commands;
mod portable_runtime_state_commands;
mod provider_commands;
mod runtime_contract;
mod runtime_generation_registry;
mod state;

use std::sync::atomic::{AtomicU32, Ordering};

use tauri::{LogicalSize, Manager, WindowEvent};
use tauri_plugin_lorepia_platform::LorepiaPlatformExt;

/// Narrowest shape the window may take, as width over height.
///
/// Recent handsets run between 19.5:9 and 20:9; the taller of the two is the
/// most elongated screen the layout is drawn for. Letting the window pass it
/// would ask every screen to render in a shape no phone has, so the minimum
/// width follows whatever height the window currently has rather than sitting
/// at one fixed number.
const NARROWEST_PHONE_ASPECT: f64 = 9.0 / 20.0;

/// Floor from `tauri.conf.json`, kept here so the two minimums move together.
/// At the 9:20 aspect floor this produces a 248 × 552 logical-point window,
/// matching the smallest user-verified layout that keeps every control intact.
const MIN_WINDOW_HEIGHT: f64 = 552.0;

/// Last minimum width handed to the window manager, in logical points.
///
/// `set_min_size` has no getter, and calling it resizes the window, which
/// raises another resize event. Remembering the last value lets the handler
/// skip the calls that would change nothing and so never feed itself.
static APPLIED_MIN_WIDTH: AtomicU32 = AtomicU32::new(0);

/// Ties the window's minimum width to its current height.
///
/// Called on every resize, so dragging the window taller also raises the width
/// it may be squeezed to.
fn hold_phone_aspect(window: &tauri::Window) {
    let Ok(scale) = window.scale_factor() else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    let logical = size.to_logical::<f64>(scale);
    let target = (logical.height * NARROWEST_PHONE_ASPECT).round();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a window dimension in points is small and never negative"
    )]
    let target_points = target.max(0.0) as u32;
    if APPLIED_MIN_WIDTH.swap(target_points, Ordering::Relaxed) == target_points {
        return;
    }
    let _ = window.set_min_size(Some(LogicalSize::new(target, MIN_WINDOW_HEIGHT)));
}

/// Starts the native `LorePia` shell and owns the process event loop.
///
/// # Panics
///
/// Panics when Tauri cannot construct or run the application. At this point
/// there is no usable application process to recover.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[allow(
    clippy::too_many_lines,
    reason = "one explicit handler list is easier to audit against capabilities and generated permissions"
)]
pub fn run() {
    let asset_admission = asset_protocol::AssetProtocolAdmission::default();
    tauri::Builder::default()
        .plugin(tauri_plugin_lorepia_platform::init())
        .register_asynchronous_uri_scheme_protocol(
            "lorepia-asset",
            move |context, request, responder| {
                if let Some(response) = asset_protocol::preflight_response(&request) {
                    responder.respond(response);
                    return;
                }
                let Some(permit) = asset_admission.try_acquire(&request) else {
                    responder.respond(asset_protocol::overloaded_response());
                    return;
                };
                let app = context.app_handle().clone();
                let _task = tauri::async_runtime::spawn_blocking(move || {
                    let mut response = asset_protocol::handle(app.state(), request);
                    asset_protocol::retain_permit_in_response(&mut response, permit);
                    responder.respond(response);
                });
            },
        )
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::Resized(_)) {
                hold_phone_aspect(window);
            }
        })
        .setup(|app| {
            for window in app.webview_windows().values() {
                hold_phone_aspect(&window.as_ref().window());
            }
            let data_root = app.lorepia_platform().data_root().to_path_buf();
            app.manage(state::AppState::new_with_app(
                data_root,
                app.handle().clone(),
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::get_memory_supervisor_status,
            commands::list_characters,
            commands::get_character,
            commands::get_character_greeting_catalog,
            commands::get_character_render_profile,
            portable_runtime_state_commands::get_portable_runtime_state,
            portable_runtime_state_commands::put_portable_runtime_state,
            commands::resolve_asset_delivery,
            package_commands::pick_content_package_import,
            package_commands::list_completed_content_package_exports,
            package_commands::export_content_source,
            package_commands::reopen_content_package_import,
            package_commands::list_pending_content_package_imports,
            package_commands::select_content_package_import,
            package_commands::approve_content_package_import,
            package_commands::commit_content_package_import,
            package_commands::discard_content_package_import,
            commands::pick_import,
            commands::inspect_import,
            commands::commit_import,
            commands::discard_import,
            commands::create_conversation,
            commands::open_conversation,
            commands::open_existing_conversation,
            commands::list_conversations,
            commands::list_conversations_for_character,
            commands::get_conversation,
            commands::get_conversation_state,
            commands::list_branches,
            commands::create_branch,
            commands::select_branch,
            commands::set_conversation_mode,
            persona_commands::create_persona,
            persona_commands::update_persona,
            persona_commands::get_persona,
            persona_commands::list_personas,
            persona_commands::list_persona_page,
            persona_commands::delete_persona,
            persona_commands::get_conversation_persona_selection,
            persona_commands::select_conversation_persona,
            persona_commands::clear_conversation_persona,
            commands::list_branch_messages,
            commands::list_messages,
            commands::generate_runtime_text,
            commands::cancel_runtime_text,
            commands::send_message,
            commands::edit_user_message,
            commands::regenerate_assistant_message,
            commands::remove_message_from_branch,
            commands::cancel_generation,
            commands::subscribe_generation,
            commands::dispose_chat_stream,
            commands::credential_status,
            commands::capture_credential,
            commands::delete_credential,
            commands::get_provider_overview,
            provider_commands::get_settings,
            provider_commands::update_settings,
            provider_commands::select_generation_target,
            provider_commands::list_provider_templates,
            provider_commands::list_provider_connections,
            provider_commands::create_provider_connection,
            provider_commands::upsert_provider_connection,
            provider_commands::delete_provider_connection,
            provider_commands::list_provider_profiles,
            commands::list_model_routes,
            provider_commands::upsert_model_route,
            provider_commands::delete_model_route,
            provider_commands::list_capability_observations,
            provider_commands::effective_capability,
            provider_commands::effective_parameter_specs,
            provider_commands::upsert_user_capability_override,
            provider_commands::delete_user_capability_override,
            commands::list_generation_presets,
            provider_commands::upsert_generation_preset,
            provider_commands::delete_generation_preset,
            provider_commands::validate_generation_preset_candidate,
            provider_commands::render_reasoning_control_for_preset,
            provider_commands::render_prompt_cache_control_for_preset,
            provider_commands::preview_provider_request_candidate,
            commands::preview_provider_request,
            provider_commands::start_provider_model_sync,
            provider_commands::get_provider_model_sync,
            provider_commands::list_provider_model_syncs,
            provider_commands::approve_provider_model_sync,
            provider_commands::cancel_provider_model_sync,
            provider_commands::poll_provider_model_sync_events,
            provider_commands::ack_provider_model_sync_event,
            provider_commands::begin_provider_discovery,
            provider_commands::begin_provider_discovery_curl,
            provider_commands::list_provider_discoveries,
            provider_commands::get_provider_discovery,
            provider_commands::list_provider_discovery_candidates,
            provider_commands::list_provider_discovery_evidence,
            provider_commands::list_provider_discovery_approvals,
            provider_commands::get_provider_discovery_review,
            provider_commands::get_provider_discovery_approval_proposal,
            provider_commands::get_provider_discovery_review_proposal,
            provider_commands::get_provider_discovery_assistant_resume_boundary,
            provider_commands::run_provider_discovery_assistant_turn,
            provider_commands::resume_provider_discovery_assistant_core_host_action,
            provider_commands::approve_provider_discovery_assistant_retry,
            provider_commands::request_provider_discovery_assistant_revision,
            provider_commands::accept_provider_discovery_assistant_draft,
            provider_commands::record_provider_discovery_assistant_failure,
            provider_commands::interrupt_provider_discovery_assistant,
            provider_commands::restart_provider_discovery_assistant_after_interruption,
            provider_commands::continue_provider_discovery,
            provider_commands::supply_provider_discovery_document_evidence,
            provider_commands::supply_provider_discovery_curl_evidence,
            provider_commands::cancel_provider_discovery,
            provider_commands::commit_provider_discovery,
            provider_commands::poll_provider_discovery_events,
            provider_commands::poll_provider_discovery_events_for_session,
            provider_commands::ack_provider_discovery_event,
            provider_commands::recover_provider_discovery,
            provider_commands::list_provider_discovery_compensation_steps,
            provider_commands::continue_provider_discovery_compensation,
            provider_commands::resume_provider_discovery_compensation,
            provider_commands::pick_provider_catalog_import,
            provider_commands::activate_provider_catalog_import,
            provider_commands::discard_provider_catalog_import,
            provider_commands::provider_catalog_status,
            provider_commands::provider_catalog_history,
            provider_commands::diff_provider_catalog_revisions,
            provider_commands::prepare_provider_catalog_rollback,
            provider_commands::activate_provider_catalog_rollback,
            orchestration_commands::validate_prompt_preset,
            orchestration_commands::resolve_prompt_preview,
            orchestration_commands::send_reviewed_prompt,
            orchestration_commands::explain_prompt_plan,
            orchestration_commands::get_orchestration_workspace,
            orchestration_commands::save_room_orchestration_config,
            orchestration_commands::upsert_prompt_preset,
            orchestration_commands::get_prompt_preset,
            orchestration_commands::get_editable_prompt_preset,
            orchestration_commands::list_prompt_presets,
            orchestration_commands::list_prompt_preset_revisions,
            orchestration_commands::diff_prompt_preset_revisions,
            orchestration_commands::review_prompt_preset_rollback,
            orchestration_commands::apply_prompt_preset_rollback,
            orchestration_commands::reorder_prompt_blocks,
            orchestration_commands::delete_prompt_preset,
            orchestration_commands::upsert_task_profile,
            orchestration_commands::get_task_profile,
            orchestration_commands::list_task_profiles,
            orchestration_commands::delete_task_profile,
            orchestration_commands::upsert_memory_profile,
            orchestration_commands::get_memory_profile,
            orchestration_commands::list_memory_profiles,
            orchestration_commands::delete_memory_profile,
            orchestration_commands::get_memory_record,
            orchestration_commands::patch_memory_record,
            orchestration_commands::set_memory_record_exclusion,
            orchestration_commands::delete_memory_record,
            orchestration_commands::upsert_knowledge_book,
            orchestration_commands::get_knowledge_book,
            orchestration_commands::list_knowledge_books,
            orchestration_commands::delete_knowledge_book,
            orchestration_commands::upsert_transform_set,
            orchestration_commands::get_transform_set,
            orchestration_commands::list_transform_sets,
            orchestration_commands::delete_transform_set,
            orchestration_commands::upsert_interaction_rule_set,
            orchestration_commands::get_interaction_rule_set,
            orchestration_commands::list_interaction_rule_sets,
            orchestration_commands::delete_interaction_rule_set,
            orchestration_commands::list_interaction_effects,
            orchestration_commands::list_interaction_proposals,
            orchestration_commands::list_generation_attempt_proposals,
            orchestration_commands::list_retryable_generation_attempts,
            orchestration_commands::expire_interaction_proposals,
            orchestration_commands::expire_generation_attempt_proposals,
            orchestration_commands::list_interaction_effect_history,
            orchestration_commands::list_reopen_interaction_effects,
            orchestration_commands::submit_interaction_choice,
            orchestration_commands::acknowledge_interaction_effect,
            orchestration_commands::retry_interaction_effect,
            orchestration_commands::decide_interaction_proposal,
            orchestration_commands::decide_generation_attempt_proposal,
            orchestration_commands::upsert_content_module,
            orchestration_commands::get_content_module,
            orchestration_commands::list_content_modules,
            orchestration_commands::delete_content_module,
            orchestration_commands::list_prompt_preset_bindings,
            orchestration_commands::list_memory_records,
            orchestration_commands::retry_interrupted_memory_job,
            orchestration_commands::list_interrupted_memory_jobs,
            orchestration_commands::list_retryable_memory_query_embeddings,
            orchestration_commands::retry_memory_query_embedding,
            orchestration_commands::simulate_knowledge_activation,
            orchestration_commands::preview_transform_rule,
            orchestration_commands::list_content_module_bindings,
            orchestration_commands::list_content_module_revisions,
            orchestration_commands::diff_content_module_revisions,
            orchestration_commands::evaluate_content_module_share,
            module_lifecycle_commands::list_content_module_lifecycle_candidates,
            module_lifecycle_commands::list_content_module_lifecycle_bindings,
            module_lifecycle_commands::review_content_module_activation,
            module_lifecycle_commands::resolve_content_module_activation,
            module_lifecycle_commands::activate_content_module,
            module_lifecycle_commands::review_content_module_deactivation,
            module_lifecycle_commands::deactivate_content_module,
            module_lifecycle_commands::review_content_module_rollback,
            module_lifecycle_commands::resolve_content_module_rollback,
            module_lifecycle_commands::apply_content_module_rollback,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run LorePia");
}
