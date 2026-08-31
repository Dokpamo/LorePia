use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Mutex, OnceLock},
};

use lorepia_shell_api as shell;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, State};
use tauri_plugin_lorepia_platform::{
    BoundCredentialObservation, CredentialAuthority, CredentialStatus, LorepiaPlatformExt,
    NativeCaptureStatus, NativeCredential, NativeCredentialEffect,
    NativeCredentialEffectConfirmation, NativeCredentialEffectContext, PlatformErrorCode,
    PlatformResult, PreparedBoundCredentialStore,
};
use uuid::Uuid;

use crate::{
    error::{CommandError, CommandResult},
    state::{AppState, CatalogImportTicket, DiscoveryCredentialLeaseBinding},
};

mod capabilities;
mod catalog;
mod connections;
mod credentials;
mod discovery;
mod model_routes;
mod model_sync;

// These named re-exports preserve the pre-split command and DTO facade. Some
// DTO names are consumed only by macro expansion or downstream crate users.
#[allow(unused_imports)]
pub use connections::{
    __cmd__create_provider_connection, __cmd__delete_provider_connection,
    __cmd__list_provider_connections, __cmd__upsert_provider_connection,
    __tauri_command_name_create_provider_connection,
    __tauri_command_name_delete_provider_connection,
    __tauri_command_name_list_provider_connections,
    __tauri_command_name_upsert_provider_connection, CreateProviderConnectionRequest,
    ProviderConnectionRequest, UpdateProviderConnectionRequest, create_provider_connection,
    delete_provider_connection, list_provider_connections, upsert_provider_connection,
};

#[allow(unused_imports)]
pub use model_routes::{
    __cmd__delete_generation_preset, __cmd__delete_model_route, __cmd__get_settings,
    __cmd__list_provider_profiles, __cmd__list_provider_templates,
    __cmd__preview_provider_request_candidate, __cmd__render_prompt_cache_control_for_preset,
    __cmd__render_reasoning_control_for_preset, __cmd__select_generation_target,
    __cmd__update_settings, __cmd__upsert_generation_preset, __cmd__upsert_model_route,
    __cmd__validate_generation_preset_candidate, __tauri_command_name_delete_generation_preset,
    __tauri_command_name_delete_model_route, __tauri_command_name_get_settings,
    __tauri_command_name_list_provider_profiles, __tauri_command_name_list_provider_templates,
    __tauri_command_name_preview_provider_request_candidate,
    __tauri_command_name_render_prompt_cache_control_for_preset,
    __tauri_command_name_render_reasoning_control_for_preset,
    __tauri_command_name_select_generation_target, __tauri_command_name_update_settings,
    __tauri_command_name_upsert_generation_preset, __tauri_command_name_upsert_model_route,
    __tauri_command_name_validate_generation_preset_candidate, GenerationPresetCandidateRequest,
    GenerationPresetRequest, ModelRouteRequest, SelectGenerationTargetRequest,
    UpdateSettingsRequest, UpsertModelRouteRequest, delete_generation_preset, delete_model_route,
    get_settings, list_provider_profiles, list_provider_templates,
    preview_provider_request_candidate, render_prompt_cache_control_for_preset,
    render_reasoning_control_for_preset, select_generation_target, update_settings,
    upsert_generation_preset, upsert_model_route, validate_generation_preset_candidate,
};

#[allow(unused_imports)]
pub use capabilities::{
    __cmd__delete_user_capability_override, __cmd__effective_capability,
    __cmd__effective_parameter_specs, __cmd__list_capability_observations,
    __cmd__upsert_user_capability_override, __tauri_command_name_delete_user_capability_override,
    __tauri_command_name_effective_capability, __tauri_command_name_effective_parameter_specs,
    __tauri_command_name_list_capability_observations,
    __tauri_command_name_upsert_user_capability_override, DeleteCapabilityOverrideRequest,
    EffectiveCapabilityRequest, UpsertCapabilityOverrideRequest, delete_user_capability_override,
    effective_capability, effective_parameter_specs, list_capability_observations,
    upsert_user_capability_override,
};

#[allow(unused_imports)]
pub use model_sync::{
    __cmd__ack_provider_model_sync_event, __cmd__approve_provider_model_sync,
    __cmd__cancel_provider_model_sync, __cmd__get_provider_model_sync,
    __cmd__list_provider_model_syncs, __cmd__poll_provider_model_sync_events,
    __cmd__start_provider_model_sync, __tauri_command_name_ack_provider_model_sync_event,
    __tauri_command_name_approve_provider_model_sync,
    __tauri_command_name_cancel_provider_model_sync, __tauri_command_name_get_provider_model_sync,
    __tauri_command_name_list_provider_model_syncs,
    __tauri_command_name_poll_provider_model_sync_events,
    __tauri_command_name_start_provider_model_sync, AckProviderModelSyncEventRequest,
    ApproveProviderModelSyncRequest, ListProviderModelSyncsRequest, ModelSyncJobRequest,
    PollProviderModelSyncEventsRequest, StartProviderModelSyncRequest,
    ack_provider_model_sync_event, approve_provider_model_sync, cancel_provider_model_sync,
    get_provider_model_sync, list_provider_model_syncs, poll_provider_model_sync_events,
    start_provider_model_sync,
};

#[allow(unused_imports)]
pub use discovery::{
    __cmd__accept_provider_discovery_assistant_draft, __cmd__ack_provider_discovery_event,
    __cmd__approve_provider_discovery_assistant_retry, __cmd__begin_provider_discovery,
    __cmd__begin_provider_discovery_curl, __cmd__cancel_provider_discovery,
    __cmd__commit_provider_discovery, __cmd__continue_provider_discovery,
    __cmd__continue_provider_discovery_compensation, __cmd__get_provider_discovery,
    __cmd__get_provider_discovery_approval_proposal,
    __cmd__get_provider_discovery_assistant_resume_boundary, __cmd__get_provider_discovery_review,
    __cmd__get_provider_discovery_review_proposal, __cmd__interrupt_provider_discovery_assistant,
    __cmd__list_provider_discoveries, __cmd__list_provider_discovery_approvals,
    __cmd__list_provider_discovery_candidates, __cmd__list_provider_discovery_compensation_steps,
    __cmd__list_provider_discovery_evidence, __cmd__poll_provider_discovery_events,
    __cmd__poll_provider_discovery_events_for_session,
    __cmd__record_provider_discovery_assistant_failure, __cmd__recover_provider_discovery,
    __cmd__request_provider_discovery_assistant_revision,
    __cmd__restart_provider_discovery_assistant_after_interruption,
    __cmd__resume_provider_discovery_assistant_core_host_action,
    __cmd__resume_provider_discovery_compensation, __cmd__run_provider_discovery_assistant_turn,
    __cmd__supply_provider_discovery_curl_evidence,
    __cmd__supply_provider_discovery_document_evidence,
    __tauri_command_name_accept_provider_discovery_assistant_draft,
    __tauri_command_name_ack_provider_discovery_event,
    __tauri_command_name_approve_provider_discovery_assistant_retry,
    __tauri_command_name_begin_provider_discovery,
    __tauri_command_name_begin_provider_discovery_curl,
    __tauri_command_name_cancel_provider_discovery, __tauri_command_name_commit_provider_discovery,
    __tauri_command_name_continue_provider_discovery,
    __tauri_command_name_continue_provider_discovery_compensation,
    __tauri_command_name_get_provider_discovery,
    __tauri_command_name_get_provider_discovery_approval_proposal,
    __tauri_command_name_get_provider_discovery_assistant_resume_boundary,
    __tauri_command_name_get_provider_discovery_review,
    __tauri_command_name_get_provider_discovery_review_proposal,
    __tauri_command_name_interrupt_provider_discovery_assistant,
    __tauri_command_name_list_provider_discoveries,
    __tauri_command_name_list_provider_discovery_approvals,
    __tauri_command_name_list_provider_discovery_candidates,
    __tauri_command_name_list_provider_discovery_compensation_steps,
    __tauri_command_name_list_provider_discovery_evidence,
    __tauri_command_name_poll_provider_discovery_events,
    __tauri_command_name_poll_provider_discovery_events_for_session,
    __tauri_command_name_record_provider_discovery_assistant_failure,
    __tauri_command_name_recover_provider_discovery,
    __tauri_command_name_request_provider_discovery_assistant_revision,
    __tauri_command_name_restart_provider_discovery_assistant_after_interruption,
    __tauri_command_name_resume_provider_discovery_assistant_core_host_action,
    __tauri_command_name_resume_provider_discovery_compensation,
    __tauri_command_name_run_provider_discovery_assistant_turn,
    __tauri_command_name_supply_provider_discovery_curl_evidence,
    __tauri_command_name_supply_provider_discovery_document_evidence,
    BeginProviderDiscoveryCurlRequest, BeginProviderDiscoveryRequest,
    CancelProviderDiscoveryRequest, CapturedProviderDiscoveryDto, CommitProviderDiscoveryRequest,
    ContinueProviderDiscoveryRequest, DiscoveryCompensationStepsRequest,
    InterruptProviderDiscoveryAssistantRequest, LimitRequest,
    PollProviderDiscoveryEventsForSessionRequest, ProviderDiscoveryEventRequest,
    ProviderDiscoverySessionRequest, RecordProviderDiscoveryAssistantFailureRequest,
    SupplyProviderDiscoveryCurlEvidenceRequest, SupplyProviderDiscoveryDocumentEvidenceRequest,
    accept_provider_discovery_assistant_draft, ack_provider_discovery_event,
    approve_provider_discovery_assistant_retry, begin_provider_discovery,
    begin_provider_discovery_curl, cancel_provider_discovery, commit_provider_discovery,
    continue_provider_discovery, continue_provider_discovery_compensation, get_provider_discovery,
    get_provider_discovery_approval_proposal, get_provider_discovery_assistant_resume_boundary,
    get_provider_discovery_review, get_provider_discovery_review_proposal,
    interrupt_provider_discovery_assistant, list_provider_discoveries,
    list_provider_discovery_approvals, list_provider_discovery_candidates,
    list_provider_discovery_compensation_steps, list_provider_discovery_evidence,
    poll_provider_discovery_events, poll_provider_discovery_events_for_session,
    record_provider_discovery_assistant_failure, recover_provider_discovery,
    request_provider_discovery_assistant_revision,
    restart_provider_discovery_assistant_after_interruption,
    resume_provider_discovery_assistant_core_host_action, resume_provider_discovery_compensation,
    run_provider_discovery_assistant_turn, supply_provider_discovery_curl_evidence,
    supply_provider_discovery_document_evidence,
};

#[allow(unused_imports)]
pub use catalog::{
    __cmd__activate_provider_catalog_import, __cmd__activate_provider_catalog_rollback,
    __cmd__diff_provider_catalog_revisions, __cmd__discard_provider_catalog_import,
    __cmd__pick_provider_catalog_import, __cmd__prepare_provider_catalog_rollback,
    __cmd__provider_catalog_history, __cmd__provider_catalog_status,
    __tauri_command_name_activate_provider_catalog_import,
    __tauri_command_name_activate_provider_catalog_rollback,
    __tauri_command_name_diff_provider_catalog_revisions,
    __tauri_command_name_discard_provider_catalog_import,
    __tauri_command_name_pick_provider_catalog_import,
    __tauri_command_name_prepare_provider_catalog_rollback,
    __tauri_command_name_provider_catalog_history, __tauri_command_name_provider_catalog_status,
    ActivateProviderCatalogRollbackRequest, PrepareProviderCatalogRollbackRequest,
    ProviderCatalogDiffRequest, ProviderCatalogHistoryRequest, ProviderCatalogImportTicketDto,
    ProviderCatalogTicketRequest, activate_provider_catalog_import,
    activate_provider_catalog_rollback, diff_provider_catalog_revisions,
    discard_provider_catalog_import, pick_provider_catalog_import,
    prepare_provider_catalog_rollback, provider_catalog_history, provider_catalog_status,
};

#[allow(unused_imports)]
pub(crate) use credentials::{
    capture_discovery_credential_for_empty_bound_slot, capture_precommit_discovery_credential,
    discovery_credential_authority, discovery_credential_reservation_authority,
    discovery_credential_status, status_only_bound_observation,
};
#[allow(unused_imports)]
pub(crate) use discovery::{
    recover_provider_discovery_backend, recover_provider_discovery_with_shell,
};

#[cfg(test)]
use connections::create_provider_connection_with_slot_guard;
#[cfg(test)]
use credentials::{
    CapturedDiscoveryCredential, CompensationCredentialEffectPolicy,
    CompensationObserveErrorPolicy, ConnectionSlotGuardFuture, CredentialCompensationDeleteOutcome,
    CredentialInstallRecoveryAction, DiscoveryCompensationConfirmation,
    DiscoveryCompensationDriveResult, DiscoveryCredentialCommitCandidate,
    DiscoveryCredentialInstallJournal, DiscoveryCredentialVault, DiscoveryVaultFuture,
    ExistingConnectionCredentialReadFuture, ExistingConnectionCredentialReader,
    NewConnectionSlotGuard, PreparedDiscoveryCredentialStore,
    capture_discovery_credential_for_empty_bound_slot_with,
    capture_precommit_discovery_credential_with, credential_compensation_delete_outcome,
    credential_for_discovery_action, credential_install_recovery_action,
    delete_and_observe_discovery_bound_slot, discovery_committing_credential_status_with,
    discovery_compensation_confirmation_context, discovery_compensation_credential_authority,
    drive_provider_discovery_compensation_with, observe_discovery_compensation_slot,
    promote_discovery_credential_lease_with, recover_provider_discovery_credential_installs,
    require_started_discovery_credential_install, settle_started_discovery_credential_recovery,
};
#[cfg(test)]
use discovery::{
    MAXIMUM_PROVIDER_CURL_BYTES, begin_provider_discovery_curl_with_reader,
    begin_provider_discovery_with_reader, bounded_secret_curl,
    continue_provider_discovery_off_runtime, register_active_discovery_request,
    request_provider_discovery_cancellation, run_shell_discovery_off_runtime,
    supply_provider_discovery_curl_evidence_off_runtime,
};
#[cfg(test)]
use model_sync::start_provider_model_sync_with_reader;
#[cfg(test)]
mod tests {
    include!("provider_commands/tests/support.rs");
    include!("provider_commands/tests/provider_access.rs");
    include!("provider_commands/tests/credential_install.rs");
    include!("provider_commands/tests/compensation.rs");
}
