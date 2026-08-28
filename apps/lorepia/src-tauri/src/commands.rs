use std::{future::Future, pin::Pin};

use lorepia_shell_api::{
    AssetDeliveryDto, BootstrapDto, CharacterDto, CharacterGreetingCatalogDto, ChatStreamItem,
    ConversationBranchDto, ConversationDto, ConversationStateDto, CreateConversationBranchInput,
    CreateConversationInput, EditUserMessageInput, GenerateRuntimeTextInput, GenerationCredential,
    GenerationPresetDto, GenerationSelectionInput, GenerationStartedDto, ImportInspectionDto,
    MessageActionGenerationDto, MessageDto, ModelRouteDto, RegenerateAssistantMessageInput,
    RemoveMessageInput, RequestPreviewDto, ResolveAssetDeliveryInput, RuntimeTextGenerationDto,
    SecretCredential, SelectConversationBranchInput, SendMessageInput, SetConversationModeInput,
    StagedImportFile,
};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, State, ipc::Channel};
use tauri_plugin_lorepia_platform::{
    BoundCredentialObservation, CredentialAuthority, CredentialStatus, LegacyCredentialObservation,
    LorepiaPlatformExt, NativeCredential, NativeCredentialEffect,
    NativeCredentialEffectConfirmation, NativeCredentialEffectContext, PlatformErrorCode,
    PlatformResult,
};
use uuid::Uuid;

use crate::{
    channels::forward_chat_stream,
    contract::{
        BranchMessagesRequest, CharacterConversationsRequest, CharacterRequest, ChatStreamRequest,
        CredentialStatusDto, CredentialStatusRequest, CredentialTarget, DiscardImportRequest,
        GenerationPresetsRequest, GenerationRequest, ImportTicketDto, InspectionRequest,
        MemorySupervisorStatusDto, ModelRoutesRequest, NativeCaptureStatusDto,
        PreviewProviderRequest, ProviderOverviewDto, SubscribeGenerationRequest, TicketRequest,
    },
    error::{CommandError, CommandResult},
    state::AppState,
};

type LegacyGenerationCredentialReadFuture<'a> =
    Pin<Box<dyn Future<Output = CommandResult<Option<NativeCredential>>> + Send + 'a>>;

trait LegacyGenerationCredentialReader: Send + Sync {
    fn read<'a>(
        &'a self,
        shell: &'a lorepia_shell_api::ShellApi,
        provider_profile_id: &'a str,
    ) -> LegacyGenerationCredentialReadFuture<'a>;
}

struct PlatformLegacyGenerationCredentialReader<'a> {
    app: &'a AppHandle,
}

impl LegacyGenerationCredentialReader for PlatformLegacyGenerationCredentialReader<'_> {
    fn read<'a>(
        &'a self,
        shell: &'a lorepia_shell_api::ShellApi,
        provider_profile_id: &'a str,
    ) -> LegacyGenerationCredentialReadFuture<'a> {
        Box::pin(async move {
            crate::credential_operations::read_legacy_provider_credential(
                self.app,
                shell,
                provider_profile_id,
            )
            .await
        })
    }
}

#[tauri::command]
pub async fn bootstrap(app: AppHandle, state: State<'_, AppState>) -> CommandResult<BootstrapDto> {
    state.bootstrap(&app).await
}

#[tauri::command]
pub fn get_memory_supervisor_status(
    state: State<'_, AppState>,
) -> CommandResult<MemorySupervisorStatusDto> {
    state.memory_supervisor_status()
}

#[tauri::command]
pub fn list_characters(state: State<'_, AppState>) -> CommandResult<Vec<CharacterDto>> {
    state.shell()?.list_characters().map_err(Into::into)
}

#[tauri::command]
pub fn get_character(
    state: State<'_, AppState>,
    request: CharacterRequest,
) -> CommandResult<CharacterDto> {
    state
        .shell()?
        .get_character(&request.character_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_character_greeting_catalog(
    state: State<'_, AppState>,
    request: CharacterRequest,
) -> CommandResult<CharacterGreetingCatalogDto> {
    state
        .shell()?
        .get_character_greeting_catalog(&request.character_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_character_render_profile(
    state: State<'_, AppState>,
    request: CharacterRequest,
) -> CommandResult<lorepia_shell_api::CharacterRenderProfileDto> {
    state
        .shell()?
        .get_character_render_profile(&request.character_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn resolve_asset_delivery(
    state: State<'_, AppState>,
    request: ResolveAssetDeliveryInput,
) -> CommandResult<AssetDeliveryDto> {
    execute_resolve_asset_delivery(&state.shell()?, request)
}

pub(crate) fn execute_resolve_asset_delivery(
    shell_api: &lorepia_shell_api::ShellApi,
    request: ResolveAssetDeliveryInput,
) -> CommandResult<AssetDeliveryDto> {
    shell_api
        .resolve_asset_delivery(request)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn pick_import(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Option<ImportTicketDto>> {
    state.ensure_ready()?;
    let Some(staged) = app.lorepia_platform().pick_import().await? else {
        return Ok(None);
    };
    let ticket_id = Uuid::new_v4().to_string();
    let response = ImportTicketDto {
        ticket_id: ticket_id.clone(),
        display_name: staged.display_name().to_owned(),
        size_bytes: staged.size_bytes(),
    };
    state.insert_import_ticket(ticket_id, staged)?;
    Ok(Some(response))
}

#[tauri::command]
pub async fn inspect_import(
    app: AppHandle,
    state: State<'_, AppState>,
    request: TicketRequest,
) -> CommandResult<ImportInspectionDto> {
    let staged = state.take_import_ticket(&request.ticket_id)?;
    let shell = state.shell()?;
    let inspection = shell
        .inspect_import(&StagedImportFile::new(staged.path()))
        .map_err(CommandError::from);
    let cleanup = app
        .lorepia_platform()
        .discard_staged_import(&staged)
        .await
        .map_err(CommandError::from);

    match (inspection, cleanup) {
        (Ok(inspection), Ok(())) => Ok(inspection),
        (Ok(inspection), Err(cleanup_error)) => {
            let _ = shell.discard_import(&inspection.inspection_id);
            Err(cleanup_error)
        }
        (Err(error), _) => Err(error),
    }
}

#[tauri::command]
pub fn commit_import(
    state: State<'_, AppState>,
    request: InspectionRequest,
) -> CommandResult<CharacterDto> {
    state
        .shell()?
        .commit_import(&request.inspection_id)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn discard_import(
    app: AppHandle,
    state: State<'_, AppState>,
    request: DiscardImportRequest,
) -> CommandResult<()> {
    match request {
        DiscardImportRequest::Inspection { inspection_id } => state
            .shell()?
            .discard_import(&inspection_id)
            .map_err(Into::into),
        DiscardImportRequest::Ticket { ticket_id } => {
            let reservation = state.reserve_import_ticket(&ticket_id)?;
            match app
                .lorepia_platform()
                .discard_staged_import(reservation.value())
                .await
            {
                Ok(()) => reservation.complete(),
                Err(error) => Err(error.into()),
            }
        }
    }
}

#[tauri::command]
pub fn create_conversation(
    state: State<'_, AppState>,
    input: CreateConversationInput,
) -> CommandResult<ConversationDto> {
    state
        .shell()?
        .create_conversation(input)
        .map_err(Into::into)
}

#[tauri::command]
pub fn open_conversation(
    state: State<'_, AppState>,
    request: CharacterRequest,
) -> CommandResult<ConversationDto> {
    state
        .shell()?
        .open_conversation(&request.character_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn open_existing_conversation(
    state: State<'_, AppState>,
    request: crate::contract::ConversationRequest,
) -> CommandResult<ConversationDto> {
    state
        .shell()?
        .open_existing_conversation(&request.conversation_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_conversations(state: State<'_, AppState>) -> CommandResult<Vec<ConversationDto>> {
    state.shell()?.list_conversations().map_err(Into::into)
}

#[tauri::command]
pub fn list_conversations_for_character(
    state: State<'_, AppState>,
    request: CharacterConversationsRequest,
) -> CommandResult<Vec<ConversationDto>> {
    state
        .shell()?
        .list_conversations_for_character(&request.character_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_conversation(
    state: State<'_, AppState>,
    request: crate::contract::ConversationRequest,
) -> CommandResult<ConversationDto> {
    state
        .shell()?
        .get_conversation(&request.conversation_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_conversation_state(
    state: State<'_, AppState>,
    request: crate::contract::ConversationRequest,
) -> CommandResult<ConversationStateDto> {
    state
        .shell()?
        .get_conversation_state(&request.conversation_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_branches(
    state: State<'_, AppState>,
    request: crate::contract::ConversationRequest,
) -> CommandResult<Vec<ConversationBranchDto>> {
    state
        .shell()?
        .list_conversation_branches(&request.conversation_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn create_branch(
    state: State<'_, AppState>,
    input: CreateConversationBranchInput,
) -> CommandResult<ConversationBranchDto> {
    state
        .shell()?
        .create_conversation_branch(input)
        .map_err(Into::into)
}

#[tauri::command]
pub fn select_branch(
    state: State<'_, AppState>,
    input: SelectConversationBranchInput,
) -> CommandResult<ConversationStateDto> {
    state
        .shell()?
        .select_conversation_branch(input)
        .map_err(Into::into)
}

#[tauri::command]
pub fn set_conversation_mode(
    state: State<'_, AppState>,
    input: SetConversationModeInput,
) -> CommandResult<ConversationStateDto> {
    state
        .shell()?
        .set_conversation_mode(input)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_branch_messages(
    state: State<'_, AppState>,
    request: BranchMessagesRequest,
) -> CommandResult<Vec<MessageDto>> {
    state
        .shell()?
        .list_branch_messages(&request.branch_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_messages(
    state: State<'_, AppState>,
    request: crate::contract::ConversationRequest,
) -> CommandResult<Vec<MessageDto>> {
    state
        .shell()?
        .list_messages(&request.conversation_id)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn generate_runtime_text(
    app: AppHandle,
    state: State<'_, AppState>,
    input: GenerateRuntimeTextInput,
) -> CommandResult<RuntimeTextGenerationDto> {
    let shell = state.shell()?;
    let dispatch_lease = generation_dispatch_lease(&state, &input.selection).await;
    let credential =
        credential_for_selection(&app, &state, &shell, &input.selection, dispatch_lease).await?;
    shell
        .generate_runtime_text(input, credential)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    input: SendMessageInput,
    stream_id: String,
    on_event: Channel<ChatStreamItem>,
) -> CommandResult<GenerationStartedDto> {
    let registration = state.register_chat_stream(&stream_id)?;
    let shell = state.shell()?;
    let dispatch_lease = generation_dispatch_lease(&state, &input.selection).await;
    let credential = credential_for_selection(
        &app,
        &state,
        &shell,
        &input.selection,
        dispatch_lease.clone(),
    )
    .await?;
    let (_cancel, cancelled) = tokio::sync::watch::channel(false);
    let started = shell
        .send_message_to_branch_async(
            input,
            credential,
            &crate::state::PlatformTaskCredentialReader {
                app,
                shell: shell.clone(),
                inherited_dispatch_lease: dispatch_lease,
            },
            cancelled,
        )
        .await?;
    let (response, stream) = started.into_parts();
    forward_chat_stream(stream, on_event, registration);
    Ok(response)
}

#[tauri::command]
pub async fn edit_user_message(
    app: AppHandle,
    state: State<'_, AppState>,
    input: EditUserMessageInput,
    stream_id: String,
    on_event: Channel<ChatStreamItem>,
) -> CommandResult<MessageActionGenerationDto> {
    let registration = state.register_chat_stream(&stream_id)?;
    let shell = state.shell()?;
    let dispatch_lease = generation_dispatch_lease(&state, &input.selection).await;
    let credential = credential_for_selection(
        &app,
        &state,
        &shell,
        &input.selection,
        dispatch_lease.clone(),
    )
    .await?;
    let (_cancel, cancelled) = tokio::sync::watch::channel(false);
    let started = shell
        .edit_user_message_async(
            input,
            credential,
            &crate::state::PlatformTaskCredentialReader {
                app,
                shell: shell.clone(),
                inherited_dispatch_lease: dispatch_lease,
            },
            cancelled,
        )
        .await?;
    let (response, stream) = started.into_parts();
    forward_chat_stream(stream, on_event, registration);
    Ok(response)
}

#[tauri::command]
pub async fn regenerate_assistant_message(
    app: AppHandle,
    state: State<'_, AppState>,
    input: RegenerateAssistantMessageInput,
    stream_id: String,
    on_event: Channel<ChatStreamItem>,
) -> CommandResult<MessageActionGenerationDto> {
    let registration = state.register_chat_stream(&stream_id)?;
    let shell = state.shell()?;
    let dispatch_lease = generation_dispatch_lease(&state, &input.selection).await;
    let credential = credential_for_selection(
        &app,
        &state,
        &shell,
        &input.selection,
        dispatch_lease.clone(),
    )
    .await?;
    let (_cancel, cancelled) = tokio::sync::watch::channel(false);
    let started = shell
        .regenerate_assistant_message_async(
            input,
            credential,
            &crate::state::PlatformTaskCredentialReader {
                app,
                shell: shell.clone(),
                inherited_dispatch_lease: dispatch_lease,
            },
            cancelled,
        )
        .await?;
    let (response, stream) = started.into_parts();
    forward_chat_stream(stream, on_event, registration);
    Ok(response)
}

#[tauri::command]
pub fn remove_message_from_branch(
    state: State<'_, AppState>,
    input: RemoveMessageInput,
) -> CommandResult<ConversationBranchDto> {
    state
        .shell()?
        .remove_message_from_branch(input)
        .map_err(Into::into)
}

#[tauri::command]
pub fn cancel_generation(
    state: State<'_, AppState>,
    request: GenerationRequest,
) -> CommandResult<()> {
    state
        .shell()?
        .cancel_generation(&request.generation_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn subscribe_generation(
    state: State<'_, AppState>,
    request: SubscribeGenerationRequest,
    stream_id: String,
    on_event: Channel<ChatStreamItem>,
) -> CommandResult<()> {
    let (stream, registration) = state.admit_generation_subscription(&request, &stream_id)?;
    forward_chat_stream(stream, on_event, registration);
    Ok(())
}

#[tauri::command]
pub fn dispose_chat_stream(
    state: State<'_, AppState>,
    request: ChatStreamRequest,
) -> CommandResult<bool> {
    state.dispose_chat_stream(&request.stream_id)
}

#[tauri::command]
pub async fn credential_status(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CredentialStatusRequest,
) -> CommandResult<CredentialStatusDto> {
    let shell = state.shell()?;
    let status = match &request.target {
        CredentialTarget::LegacyProfile {
            provider_profile_id,
        } => {
            let reference = provider_profile_reference(&shell, provider_profile_id)?;
            match shell.ensure_legacy_profile_raw_credential_access(&reference) {
                Ok(()) => status_only_legacy_observation(
                    app.lorepia_platform()
                        .observe_legacy_credential(&reference)
                        .await,
                ),
                Err(error) if error.code == lorepia_shell_api::ShellErrorCode::InvalidInput => {
                    CredentialStatus::Unreadable
                }
                Err(error) => return Err(error.into()),
            }
        }
        CredentialTarget::Connection { connection_id } => {
            let connection = shell
                .list_provider_connections()?
                .into_iter()
                .find(|connection| connection.id == *connection_id)
                .ok_or_else(CommandError::invalid_input)?;
            if !connection.credential_binding_required {
                return Err(CommandError::invalid_input());
            }
            if shell.provider_connection_uses_legacy_raw_credential(connection_id)? {
                return Ok(CredentialStatusDto {
                    status: CredentialStatus::Unreadable,
                });
            }
            match status_only_connection_access(&shell, connection_id)? {
                StatusOnlyConnectionAccess::Settled(authority) => {
                    crate::provider_commands::status_only_bound_observation(
                        app.lorepia_platform()
                            .observe_bound_credential(connection_id, &authority)
                            .await,
                    )
                }
                StatusOnlyConnectionAccess::Unowned => status_only_unowned_observation(
                    app.lorepia_platform()
                        .credential_status(connection_id)
                        .await,
                ),
                StatusOnlyConnectionAccess::Unreadable => CredentialStatus::Unreadable,
            }
        }
        CredentialTarget::DiscoverySession {
            session_id,
            expected_revision,
        } => {
            crate::provider_commands::discovery_credential_status(
                &app,
                &state,
                &shell,
                session_id,
                *expected_revision,
            )
            .await?
        }
    };
    Ok(CredentialStatusDto { status })
}

enum StatusOnlyConnectionAccess {
    Settled(CredentialAuthority),
    Unowned,
    Unreadable,
}

/// Resolves only non-secret authority for a status projection.
///
/// `InvalidInput` from the settled-access guard is intentionally not enough
/// to call a slot missing: a prepared or otherwise unresolved install has no
/// prior authority either. The durable unresolved list distinguishes that
/// state from a genuinely fresh or removed connection before the raw native
/// slot is observed.
fn status_only_connection_access(
    shell: &lorepia_shell_api::ShellApi,
    connection_id: &str,
) -> CommandResult<StatusOnlyConnectionAccess> {
    match shell.ensure_provider_credential_access_settled(connection_id) {
        Ok(authority) => Ok(StatusOnlyConnectionAccess::Settled(
            CredentialAuthority::new(authority.authority_id, authority.connection_binding_sha256)?,
        )),
        Err(error) if error.code == lorepia_shell_api::ShellErrorCode::InvalidInput => {
            if shell
                .list_unresolved_provider_credential_operations()?
                .iter()
                .any(|operation| operation.connection_id == connection_id)
            {
                return Ok(StatusOnlyConnectionAccess::Unreadable);
            }
            match shell.provider_credential_recovery_authority(connection_id) {
                Ok(None) => Ok(StatusOnlyConnectionAccess::Unowned),
                Ok(Some(_))
                | Err(lorepia_shell_api::ShellError {
                    code: lorepia_shell_api::ShellErrorCode::InvalidInput,
                    ..
                }) => Ok(StatusOnlyConnectionAccess::Unreadable),
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn status_only_unowned_observation(
    observation: PlatformResult<CredentialStatus>,
) -> CredentialStatus {
    match observation {
        Ok(CredentialStatus::Missing) => CredentialStatus::Missing,
        Ok(CredentialStatus::Available | CredentialStatus::Unreadable) | Err(_) => {
            CredentialStatus::Unreadable
        }
    }
}

fn status_only_legacy_observation(
    observation: PlatformResult<LegacyCredentialObservation>,
) -> CredentialStatus {
    match observation {
        Ok(LegacyCredentialObservation::Missing) => CredentialStatus::Missing,
        Ok(LegacyCredentialObservation::Raw) => CredentialStatus::Available,
        Ok(LegacyCredentialObservation::Bound | LegacyCredentialObservation::Unreadable)
        | Err(_) => CredentialStatus::Unreadable,
    }
}

#[tauri::command]
pub async fn capture_credential(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CredentialStatusRequest,
) -> CommandResult<NativeCaptureStatusDto> {
    let shell = state.shell()?;
    if let CredentialTarget::DiscoverySession {
        session_id,
        expected_revision,
    } = &request.target
    {
        return capture_discovery_credential(&app, &state, &shell, session_id, *expected_revision)
            .await;
    }
    match &request.target {
        CredentialTarget::Connection { connection_id } => {
            let confirmation = confirm_connection_credential_effect(
                &app,
                &shell,
                connection_id,
                NativeCredentialEffect::CaptureOrReplace,
            )
            .await?;
            // The trusted modal must never hold the global credential writer:
            // an abandoned prompt would otherwise block every read/recovery.
            let _provider_operation = state.lock_provider_credential_operation().await;
            crate::credential_operations::capture_provider_connection_credential(
                &app,
                &shell,
                connection_id,
                confirmation,
            )
            .await
        }
        CredentialTarget::LegacyProfile {
            provider_profile_id,
        } => {
            let reference = provider_profile_reference(&shell, provider_profile_id)?;
            shell.ensure_legacy_profile_credential_mutation_settled(&reference)?;
            let confirmation = confirm_legacy_credential_effect(
                &app,
                &shell,
                &reference,
                NativeCredentialEffect::CaptureOrReplace,
            )
            .await?;
            let _legacy_operation = state.lock_legacy_credential_admission().await;
            if provider_profile_reference(&shell, provider_profile_id)? != reference {
                return Err(CommandError::invalid_input());
            }
            shell.ensure_legacy_profile_credential_mutation_settled(&reference)?;
            crate::credential_operations::capture_legacy_provider_credential(
                &app,
                &shell,
                &reference,
                confirmation,
            )
            .await
        }
        CredentialTarget::DiscoverySession { .. } => Err(CommandError::internal()),
    }
}

#[allow(clippy::too_many_lines)] // Keeps the confirmation-to-WAL cutpoint sequence linear.
async fn capture_discovery_credential(
    app: &AppHandle,
    state: &AppState,
    shell: &lorepia_shell_api::ShellApi,
    session_id: &str,
    expected_revision: u64,
) -> CommandResult<NativeCaptureStatusDto> {
    let session = shell.get_provider_discovery(session_id)?;
    if session.revision != expected_revision || !session.credential_binding_requested {
        return Err(CommandError::invalid_input());
    }
    let credential_authority = shell.get_provider_discovery_credential_lease_context(session_id)?;
    let confirmation = app
        .lorepia_platform()
        .confirm_credential_effect(discovery_capture_confirmation_context(
            &session,
            &credential_authority,
        )?)
        .await?;
    let latest = shell.get_provider_discovery(session_id)?;
    let latest_authority = shell.get_provider_discovery_credential_lease_context(session_id)?;
    if latest != session || latest_authority != credential_authority {
        return Err(CommandError::invalid_input());
    }
    let _provider_operation = state.lock_provider_credential_operation().await;
    if shell.get_provider_discovery(session_id)? != session
        || shell.get_provider_discovery_credential_lease_context(session_id)?
            != credential_authority
    {
        return Err(CommandError::invalid_input());
    }
    consume_discovery_capture_confirmation(confirmation, &session, &credential_authority)?;
    if session.state != "committing" {
        return crate::provider_commands::capture_precommit_discovery_credential(
            app,
            state,
            shell,
            session_id,
            expected_revision,
        )
        .await;
    }
    let preflight = shell.get_provider_discovery_credential_install_context(session_id)?;
    if preflight.session_revision != expected_revision
        || session.commit_attempt_id.as_deref() != Some(preflight.commit_attempt_id.as_str())
        || session.commit_plan_sha256.as_deref() != Some(preflight.commit_plan_sha256.as_str())
        || preflight.commit_phase != "prepared"
        || preflight.operation_status != "prepared"
        || preflight.native_execution_reservation_id.is_some()
        || preflight.native_execution_id.is_some()
    {
        return Err(CommandError::invalid_input());
    }
    let reserved = shell.reserve_provider_discovery_credential_install(
        session_id,
        expected_revision,
        &preflight.operation_id,
        &preflight.commit_attempt_id,
        &preflight.commit_plan_sha256,
    )?;
    validate_reserved_discovery_capture_context(
        session_id,
        expected_revision,
        &preflight,
        &reserved,
    )?;
    let authority =
        crate::provider_commands::discovery_credential_reservation_authority(&reserved)?;
    let (capture, captured) =
        crate::provider_commands::capture_discovery_credential_for_empty_bound_slot(
            app,
            &reserved.connection_id,
            &authority,
        )
        .await?;
    let reservation_id = reserved
        .native_execution_reservation_id
        .as_deref()
        .ok_or_else(CommandError::internal)?;
    let prepared_store = app.lorepia_platform().prepare_bound_credential_store(
        &reserved.connection_id,
        captured,
        &authority,
    )?;
    // Capture and every other fallible precondition are complete while Core
    // is still Prepared. Started is the store-attempt cutpoint, so the exact
    // reserved slot is written immediately after this transition.
    let context = shell.start_provider_discovery_credential_install(
        session_id,
        expected_revision,
        &reserved.operation_id,
        &reserved.commit_attempt_id,
        &reserved.commit_plan_sha256,
        reservation_id,
    )?;
    if !discovery_capture_start_is_exact(&context, &reserved, reservation_id, &authority) {
        return Err(CommandError::internal());
    }
    let store_result = app
        .lorepia_platform()
        .store_prepared_bound_credential(prepared_store)
        .await;
    finish_discovery_credential_store_with_observation(
        shell,
        state,
        session_id,
        &context,
        capture,
        store_result,
        || async {
            app.lorepia_platform()
                .observe_bound_credential(&reserved.connection_id, &authority)
                .await
        },
    )
    .await
}

fn validate_reserved_discovery_capture_context(
    session_id: &str,
    expected_revision: u64,
    preflight: &lorepia_shell_api::ProviderDiscoveryCredentialInstallContextDto,
    reserved: &lorepia_shell_api::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<()> {
    if reserved.session_id != session_id
        || reserved.session_revision != expected_revision
        || reserved.commit_attempt_id != preflight.commit_attempt_id
        || reserved.commit_plan_sha256 != preflight.commit_plan_sha256
        || reserved.commit_phase != "prepared"
        || reserved.operation_status != "prepared"
        || reserved.operation_id != preflight.operation_id
        || reserved.connection_id != preflight.connection_id
        || reserved.connection_binding_sha256 != preflight.connection_binding_sha256
        || reserved.native_execution_id.is_some()
    {
        return Err(CommandError::internal());
    }
    Ok(())
}

fn discovery_capture_start_is_exact(
    started: &lorepia_shell_api::ProviderDiscoveryCredentialInstallContextDto,
    reserved: &lorepia_shell_api::ProviderDiscoveryCredentialInstallContextDto,
    reservation_id: &str,
    authority: &CredentialAuthority,
) -> bool {
    started.session_id == reserved.session_id
        && started.session_revision == reserved.session_revision
        && started.operation_id == reserved.operation_id
        && started.operation_status == "started"
        && started.native_execution_reservation_id.as_deref() == Some(reservation_id)
        && started.native_execution_id.as_deref() == Some(authority.authority_id())
        && started.commit_attempt_id == reserved.commit_attempt_id
        && started.commit_plan_sha256 == reserved.commit_plan_sha256
        && started.commit_phase == reserved.commit_phase
        && started.connection_id == reserved.connection_id
        && started.connection_binding_sha256 == reserved.connection_binding_sha256
}

fn finish_discovery_credential_capture(
    shell: &lorepia_shell_api::ShellApi,
    state: &AppState,
    session_id: &str,
    context: &lorepia_shell_api::ProviderDiscoveryCredentialInstallContextDto,
    capture: NativeCaptureStatusDto,
    store_result: PlatformResult<()>,
    observation: PlatformResult<BoundCredentialObservation>,
) -> CommandResult<NativeCaptureStatusDto> {
    if matches!(
        &store_result,
        Err(error) if error.code() == PlatformErrorCode::CredentialRecoveryRequired
    ) {
        return settle_direct_discovery_credential_durability_unknown(
            shell,
            state,
            session_id,
            context,
            store_result,
        );
    }
    match observation {
        Ok(BoundCredentialObservation::Match) => {
            state.clear_discovery_credential_lease(session_id);
            Ok(capture)
        }
        Ok(BoundCredentialObservation::Missing) => {
            shell.attest_provider_discovery_credential_install_no_effect(
                session_id,
                &context.operation_id,
                &context.commit_attempt_id,
                &context.commit_plan_sha256,
                context
                    .native_execution_id
                    .as_deref()
                    .ok_or_else(CommandError::internal)?,
            )?;
            store_result?;
            Err(CommandError::internal())
        }
        Ok(
            BoundCredentialObservation::Legacy
            | BoundCredentialObservation::Mismatch
            | BoundCredentialObservation::Unreadable,
        ) => {
            store_result?;
            Err(CommandError::internal())
        }
        Err(error) => {
            store_result?;
            Err(error.into())
        }
    }
}

async fn finish_discovery_credential_store_with_observation<Observe, ObservationFuture>(
    shell: &lorepia_shell_api::ShellApi,
    state: &AppState,
    session_id: &str,
    context: &lorepia_shell_api::ProviderDiscoveryCredentialInstallContextDto,
    capture: NativeCaptureStatusDto,
    store_result: PlatformResult<()>,
    observe: Observe,
) -> CommandResult<NativeCaptureStatusDto>
where
    Observe: FnOnce() -> ObservationFuture,
    ObservationFuture: Future<Output = PlatformResult<BoundCredentialObservation>>,
{
    if matches!(
        &store_result,
        Err(error) if error.code() == PlatformErrorCode::CredentialRecoveryRequired
    ) {
        return settle_direct_discovery_credential_durability_unknown(
            shell,
            state,
            session_id,
            context,
            store_result,
        );
    }
    finish_discovery_credential_capture(
        shell,
        state,
        session_id,
        context,
        capture,
        store_result,
        observe().await,
    )
}

fn settle_direct_discovery_credential_durability_unknown(
    shell: &lorepia_shell_api::ShellApi,
    state: &AppState,
    session_id: &str,
    context: &lorepia_shell_api::ProviderDiscoveryCredentialInstallContextDto,
    store_result: PlatformResult<()>,
) -> CommandResult<NativeCaptureStatusDto> {
    let native_execution_id = context
        .native_execution_id
        .as_deref()
        .ok_or_else(CommandError::internal)?;
    shell.mark_provider_discovery_credential_install_durability_unknown(
        session_id,
        context.session_revision,
        &context.operation_id,
        &context.commit_attempt_id,
        &context.commit_plan_sha256,
        native_execution_id,
        &context.connection_id,
        &context.connection_binding_sha256,
    )?;
    state.clear_discovery_credential_lease(session_id);
    Err(store_result
        .expect_err("recovery-required result is an error")
        .into())
}

#[tauri::command]
pub async fn delete_credential(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CredentialStatusRequest,
) -> CommandResult<()> {
    let shell = state.shell()?;
    if matches!(&request.target, CredentialTarget::DiscoverySession { .. }) {
        // A discovery slot is owned by the durable commit/compensation recipe.
        // Direct deletion could race publication and create an untracked
        // missing credential after a successful commit.
        return Err(CommandError::invalid_input());
    }
    match &request.target {
        CredentialTarget::Connection { connection_id } => {
            let confirmation = confirm_connection_credential_effect(
                &app,
                &shell,
                connection_id,
                NativeCredentialEffect::Delete,
            )
            .await?;
            let _provider_operation = state.lock_provider_credential_operation().await;
            crate::credential_operations::delete_provider_connection_credential(
                &app,
                &shell,
                connection_id,
                confirmation,
            )
            .await
        }
        CredentialTarget::LegacyProfile {
            provider_profile_id,
        } => {
            let reference = provider_profile_reference(&shell, provider_profile_id)?;
            shell.ensure_legacy_profile_credential_mutation_settled(&reference)?;
            let confirmation = confirm_legacy_credential_effect(
                &app,
                &shell,
                &reference,
                NativeCredentialEffect::Delete,
            )
            .await?;
            let _legacy_operation = state.lock_legacy_credential_admission().await;
            if provider_profile_reference(&shell, provider_profile_id)? != reference {
                return Err(CommandError::invalid_input());
            }
            shell.ensure_legacy_profile_credential_mutation_settled(&reference)?;
            crate::credential_operations::delete_legacy_provider_credential(
                &app,
                &shell,
                &reference,
                confirmation,
            )
            .await
        }
        CredentialTarget::DiscoverySession { .. } => Err(CommandError::invalid_input()),
    }
}

async fn confirm_connection_credential_effect(
    app: &AppHandle,
    shell: &lorepia_shell_api::ShellApi,
    connection_id: &str,
    effect: NativeCredentialEffect,
) -> CommandResult<NativeCredentialEffectConfirmation> {
    let context = crate::credential_operations::provider_connection_credential_effect_context(
        shell,
        connection_id,
        effect,
    )?;
    let confirmation = app
        .lorepia_platform()
        .confirm_credential_effect(context)
        .await?;
    let latest = crate::credential_operations::provider_connection_credential_effect_context(
        shell,
        connection_id,
        effect,
    )?;
    if confirmation.context() != &latest {
        return Err(CommandError::invalid_input());
    }
    Ok(confirmation)
}

pub(crate) async fn confirm_legacy_credential_effect(
    app: &AppHandle,
    shell: &lorepia_shell_api::ShellApi,
    provider_profile_id: &str,
    effect: NativeCredentialEffect,
) -> CommandResult<NativeCredentialEffectConfirmation> {
    let context = legacy_credential_effect_context(app, shell, provider_profile_id, effect).await?;
    let confirmation = app
        .lorepia_platform()
        .confirm_credential_effect(context)
        .await?;
    let latest = legacy_credential_effect_context(app, shell, provider_profile_id, effect).await?;
    if confirmation.context() != &latest {
        return Err(CommandError::invalid_input());
    }
    Ok(confirmation)
}

async fn legacy_credential_effect_context(
    app: &AppHandle,
    shell: &lorepia_shell_api::ShellApi,
    provider_profile_id: &str,
    effect: NativeCredentialEffect,
) -> CommandResult<NativeCredentialEffectContext> {
    let profile = shell
        .list_provider_profiles()?
        .into_iter()
        .find(|profile| profile.id == provider_profile_id)
        .ok_or_else(CommandError::invalid_input)?;
    shell.ensure_legacy_profile_credential_mutation_settled(provider_profile_id)?;
    let revision = app
        .lorepia_platform()
        .legacy_credential_confirmation_revision(provider_profile_id)
        .await?;
    NativeCredentialEffectContext::new(effect, profile.id, profile.base_url, revision)
        .map_err(Into::into)
}

fn discovery_capture_confirmation_context(
    session: &lorepia_shell_api::ProviderDiscoverySessionDto,
    authority: &lorepia_shell_api::ProviderDiscoveryCredentialLeaseContextDto,
) -> CommandResult<NativeCredentialEffectContext> {
    if authority.session_id != session.id || authority.connection_id != session.connection_id {
        return Err(CommandError::invalid_input());
    }
    let revision = discovery_credential_confirmation_revision(session, authority);
    NativeCredentialEffectContext::new(
        NativeCredentialEffect::CaptureOrReplace,
        session.connection_id.clone(),
        authority.credential_api_origin.clone(),
        revision,
    )
    .map_err(Into::into)
}

fn discovery_credential_confirmation_revision(
    session: &lorepia_shell_api::ProviderDiscoverySessionDto,
    authority: &lorepia_shell_api::ProviderDiscoveryCredentialLeaseContextDto,
) -> String {
    let mut hasher = Sha256::new();
    let session_revision = session.revision.to_string();
    for value in [
        b"dev.lorepia.discovery-credential-confirmation.v1".as_slice(),
        session.id.as_bytes(),
        session_revision.as_bytes(),
        authority.session_id.as_bytes(),
        authority.connection_id.as_bytes(),
        authority.credential_api_origin.as_bytes(),
        authority.credential_origin_approval_id.as_bytes(),
        authority.credential_origin_grant_sha256.as_bytes(),
        authority.connection_binding_sha256.as_bytes(),
    ] {
        hasher.update(value);
        hasher.update([0]);
    }
    format!(
        "session_revision={};credential_authority_sha256={:x}",
        session.revision,
        hasher.finalize()
    )
}

fn consume_discovery_capture_confirmation(
    confirmation: NativeCredentialEffectConfirmation,
    session: &lorepia_shell_api::ProviderDiscoverySessionDto,
    authority: &lorepia_shell_api::ProviderDiscoveryCredentialLeaseContextDto,
) -> CommandResult<()> {
    let expected = discovery_capture_confirmation_context(session, authority)?;
    confirmation.consume_exact(
        expected.effect(),
        expected.target_id(),
        expected.origin(),
        expected.revision(),
    )?;
    Ok(())
}

#[tauri::command]
pub fn get_provider_overview(state: State<'_, AppState>) -> CommandResult<ProviderOverviewDto> {
    let shell = state.shell()?;
    Ok(ProviderOverviewDto {
        settings: shell.get_settings()?,
        templates: shell.list_provider_templates()?,
        connections: shell.list_provider_connections()?,
        legacy_profiles: shell.list_provider_profiles()?,
    })
}

#[tauri::command]
pub fn list_model_routes(
    state: State<'_, AppState>,
    request: ModelRoutesRequest,
) -> CommandResult<Vec<ModelRouteDto>> {
    state
        .shell()?
        .list_model_routes(&request.connection_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_generation_presets(
    state: State<'_, AppState>,
    request: GenerationPresetsRequest,
) -> CommandResult<Vec<GenerationPresetDto>> {
    state
        .shell()?
        .list_generation_presets(&request.model_route_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn preview_provider_request(
    state: State<'_, AppState>,
    request: PreviewProviderRequest,
) -> CommandResult<RequestPreviewDto> {
    state
        .shell()?
        .preview_provider_request(request.target)
        .map_err(Into::into)
}

pub(crate) async fn credential_for_selection(
    app: &AppHandle,
    state: &AppState,
    shell: &lorepia_shell_api::ShellApi,
    selection: &GenerationSelectionInput,
    dispatch_lease: Option<lorepia_shell_api::TaskCredentialLease>,
) -> CommandResult<GenerationCredential> {
    match selection {
        GenerationSelectionInput::LegacyProfile {
            provider_profile_id,
        } => {
            legacy_credential_for_selection_with_reader(
                state,
                shell,
                provider_profile_id,
                &PlatformLegacyGenerationCredentialReader { app },
            )
            .await
        }
        GenerationSelectionInput::Target { target } => {
            let dispatch_lease = match dispatch_lease {
                Some(lease) => lease,
                None => lorepia_shell_api::TaskCredentialLease::new(
                    state.lease_provider_credential_operation().await,
                ),
            };
            let (connection_id, credential_binding_required) =
                connection_for_route(shell, &target.model_route_id)?;
            if credential_binding_required {
                let read = crate::credential_operations::read_provider_connection_credential(
                    app,
                    shell,
                    &connection_id,
                )
                .await?;
                Ok(
                    GenerationCredential::connection_with_access_authority_and_dispatch_lease(
                        connection_id,
                        read.credential
                            .map(|value| SecretCredential::new(value.into_secret_string())),
                        read.access_authority,
                        dispatch_lease,
                    ),
                )
            } else {
                Ok(GenerationCredential::connection_with_dispatch_lease(
                    connection_id,
                    None,
                    dispatch_lease,
                ))
            }
        }
    }
}

pub(crate) async fn generation_dispatch_lease(
    state: &AppState,
    selection: &GenerationSelectionInput,
) -> Option<lorepia_shell_api::TaskCredentialLease> {
    if matches!(selection, GenerationSelectionInput::Target { .. }) {
        Some(lorepia_shell_api::TaskCredentialLease::new(
            state.lease_provider_credential_operation().await,
        ))
    } else {
        None
    }
}

async fn legacy_credential_for_selection_with_reader<
    R: LegacyGenerationCredentialReader + ?Sized,
>(
    state: &AppState,
    shell: &lorepia_shell_api::ShellApi,
    provider_profile_id: &str,
    reader: &R,
) -> CommandResult<GenerationCredential> {
    let admission_lease = state.lease_legacy_credential_admission().await;
    let reference = provider_profile_reference(shell, provider_profile_id)?;
    let credential = reader
        .read(shell, &reference)
        .await?
        .map(|value| SecretCredential::new(value.into_secret_string()));
    Ok(GenerationCredential::legacy_with_admission_lease(
        credential,
        admission_lease,
    ))
}

fn provider_profile_reference(
    shell: &lorepia_shell_api::ShellApi,
    provider_profile_id: &str,
) -> CommandResult<String> {
    shell
        .list_provider_profiles()?
        .into_iter()
        .find(|profile| profile.id == provider_profile_id)
        .map(|profile| profile.id)
        .ok_or_else(CommandError::invalid_input)
}

fn connection_for_route(
    shell: &lorepia_shell_api::ShellApi,
    route_id: &str,
) -> CommandResult<(String, bool)> {
    for connection in shell.list_provider_connections()? {
        if shell
            .list_model_routes(&connection.id)?
            .iter()
            .any(|route| route.id == route_id)
        {
            return Ok((connection.id, connection.credential_binding_required));
        }
    }
    Err(CommandError::invalid_input())
}

#[cfg(test)]
mod credential_status_tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use lorepia_shell_api::{
        CreateProviderConnectionInput, ProviderCredentialOperationKindInput,
        ProviderCredentialSlotStatusInput, ProviderDiscoveryCredentialInstallContextDto,
        ProviderNetworkModeInput, ShellApi,
    };
    use tauri_plugin_lorepia_platform::{
        BoundCredentialObservation, ClipboardCleanupStatus, CredentialAuthority, CredentialStatus,
        LegacyCredentialObservation, NativeCaptureStatus, NativeCredential, PlatformError,
        PlatformErrorCode,
    };
    use tempfile::tempdir;

    use super::{
        LegacyGenerationCredentialReadFuture, LegacyGenerationCredentialReader,
        StatusOnlyConnectionAccess, discovery_capture_confirmation_context,
        discovery_capture_start_is_exact, finish_discovery_credential_capture,
        finish_discovery_credential_store_with_observation,
        legacy_credential_for_selection_with_reader, status_only_connection_access,
        status_only_legacy_observation, status_only_unowned_observation,
        validate_reserved_discovery_capture_context,
    };
    use crate::{
        error::CommandError,
        state::{AppState, DiscoveryCredentialLeaseBinding},
    };

    struct FakeLegacyGenerationCredentialReader {
        value: Mutex<Option<NativeCredential>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[test]
    fn discovery_capture_confirmation_displays_backend_credential_api_origin() {
        let root = tempdir().expect("temporary root");
        let shell = ShellApi::open_data_root(root.path()).expect("open Shell");
        let selecting = shell
            .begin_provider_discovery(lorepia_shell_api::BeginProviderDiscoveryInput {
                connection_id: "divergent-confirmation-origin".to_owned(),
                display_name: "Divergent confirmation origin".to_owned(),
                site_url: "https://docs.example/".to_owned(),
                docs_url: None,
                credential_binding_requested: true,
                preferred_assistant: None,
                connection_options: lorepia_shell_api::ProviderDiscoveryConnectionOptionsInput {
                    values: Vec::new(),
                    api_base_path: None,
                    timeout_seconds: 30,
                    network_mode: ProviderNetworkModeInput::Public,
                    local_network_approval: None,
                },
                supplied_evidence_ids: Vec::new(),
                source: lorepia_shell_api::BeginProviderDiscoverySourceInput::KnownProvider {
                    template_id: "openrouter-v1".to_owned(),
                },
            })
            .expect("begin known-provider discovery");
        let candidate = shell
            .list_provider_discovery_candidates(&selecting.id)
            .expect("list provider candidates")
            .into_iter()
            .find(|candidate| {
                matches!(
                    &candidate.summary,
                    lorepia_shell_api::DiscoveryCandidateSummaryDto::ProviderTemplate {
                        template_id,
                        ..
                    } if template_id == "openrouter-v1"
                )
            })
            .expect("OpenRouter candidate");
        let session = shell
            .continue_provider_discovery(
                lorepia_shell_api::ContinueProviderDiscoveryInput {
                    session_id: selecting.id,
                    action_id: "00000000-0000-4000-8000-000000000071".to_owned(),
                    expected_revision: selecting.revision,
                    action:
                        lorepia_shell_api::ContinueProviderDiscoveryActionInput::SelectTemplate {
                            candidate_id: candidate.id,
                        },
                },
                None,
            )
            .expect("select OpenRouter template");
        let authority = shell
            .get_provider_discovery_credential_lease_context(&session.id)
            .expect("load backend credential authority");
        let context = discovery_capture_confirmation_context(&session, &authority)
            .expect("build trusted confirmation");

        assert_eq!(session.site_url, "https://docs.example/");
        assert_eq!(
            context.origin(),
            "https://openrouter.ai",
            "trusted native UI must display the API origin that will receive the credential"
        );
        let trusted_revision = context.revision().to_owned();
        let mut substituted_grant = authority.clone();
        substituted_grant.credential_origin_grant_sha256 = "f".repeat(64);
        assert_ne!(
            discovery_capture_confirmation_context(&session, &substituted_grant)
                .expect("build substituted-grant context")
                .revision(),
            trusted_revision,
            "a confirmation receipt cannot be replayed with a substituted origin grant"
        );
        let mut substituted_binding = authority.clone();
        substituted_binding.connection_binding_sha256 = "e".repeat(64);
        assert_ne!(
            discovery_capture_confirmation_context(&session, &substituted_binding)
                .expect("build substituted-binding context")
                .revision(),
            trusted_revision,
            "a confirmation receipt cannot be replayed with a substituted connection binding"
        );

        let mut same_origin_site = session.clone();
        same_origin_site.site_url = "https://openrouter.ai/".to_owned();
        let same_origin_context =
            discovery_capture_confirmation_context(&same_origin_site, &authority)
                .expect("same-origin capture remains valid");
        assert_eq!(same_origin_context.origin(), "https://openrouter.ai");
    }

    impl LegacyGenerationCredentialReader for FakeLegacyGenerationCredentialReader {
        fn read<'a>(
            &'a self,
            _shell: &'a ShellApi,
            provider_profile_id: &'a str,
        ) -> LegacyGenerationCredentialReadFuture<'a> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .expect("fake legacy reader calls")
                    .push(provider_profile_id.to_owned());
                self.value
                    .lock()
                    .expect("fake legacy reader")
                    .take()
                    .ok_or_else(CommandError::invalid_input)
                    .map(Some)
            })
        }
    }

    #[test]
    fn direct_discovery_capture_requires_reserved_b_then_exact_started_b() {
        let preflight = ProviderDiscoveryCredentialInstallContextDto {
            session_id: "direct-capture-session".to_owned(),
            session_revision: 9,
            operation_id: "direct-capture-operation".to_owned(),
            operation_status: "prepared".to_owned(),
            native_execution_reservation_id: None,
            native_execution_id: None,
            commit_attempt_id: "00000000-0000-4000-8000-000000000091".to_owned(),
            commit_plan_sha256: "a".repeat(64),
            commit_phase: "prepared".to_owned(),
            connection_id: "direct-capture-connection".to_owned(),
            connection_binding_sha256: "b".repeat(64),
        };
        let mut reserved = preflight.clone();
        reserved.native_execution_reservation_id = Some("native-execution-B".to_owned());
        validate_reserved_discovery_capture_context(
            &preflight.session_id,
            preflight.session_revision,
            &preflight,
            &reserved,
        )
        .expect("exact reserved Prepared B");
        let authority = CredentialAuthority::new(
            "native-execution-B".to_owned(),
            reserved.connection_binding_sha256.clone(),
        )
        .expect("B authority");
        let mut started = reserved.clone();
        started.operation_status = "started".to_owned();
        started.native_execution_id = Some("native-execution-B".to_owned());
        assert!(discovery_capture_start_is_exact(
            &started,
            &reserved,
            "native-execution-B",
            &authority,
        ));

        started.native_execution_id = Some("native-execution-A".to_owned());
        assert!(
            !discovery_capture_start_is_exact(
                &started,
                &reserved,
                "native-execution-B",
                &authority,
            ),
            "direct capture must reject stale or forged physical execution authority"
        );
    }

    #[test]
    fn direct_capture_recovery_required_settles_unknown_before_observation_and_lease_clear() {
        let root = tempdir().expect("temporary root");
        let fixture =
            lorepia_shell_api::test_support::seed_synthetic_started_discovery_credential_install(
                root.path(),
            )
            .expect("seed Started direct-capture fixture");
        let shell = fixture.shell;
        let context = fixture.install;
        let binding = DiscoveryCredentialLeaseBinding {
            session_id: fixture.lease.session_id,
            connection_id: fixture.lease.connection_id,
            credential_origin_approval_id: fixture.lease.credential_origin_approval_id,
            credential_origin_grant_sha256: fixture.lease.credential_origin_grant_sha256,
            connection_binding_sha256: fixture.lease.connection_binding_sha256,
        };
        let state = AppState::new(root.path().to_path_buf());
        state
            .install_discovery_credential_lease(
                binding.clone(),
                NativeCredential::new("synthetic-direct-capture-secret".to_owned()),
            )
            .expect("install runtime direct-capture lease");
        let capture = NativeCaptureStatus {
            clipboard_cleanup: ClipboardCleanupStatus::Cleared,
        };

        let mut drifted = context.clone();
        drifted.session_revision += 1;
        finish_discovery_credential_capture(
            &shell,
            &state,
            &context.session_id,
            &drifted,
            capture,
            Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            )),
            Ok(BoundCredentialObservation::Match),
        )
        .expect_err("stale settlement must fail closed");
        assert_eq!(
            state.discovery_credential_lease_status(&binding),
            CredentialStatus::Available,
            "lease clear is downstream of exact durable settlement"
        );
        assert_eq!(
            shell
                .get_provider_discovery(&context.session_id)
                .expect("reload stale settlement")
                .state,
            "committing"
        );

        let observation_called = Arc::new(AtomicBool::new(false));
        let observation_marker = Arc::clone(&observation_called);
        let error =
            tauri::async_runtime::block_on(finish_discovery_credential_store_with_observation(
                &shell,
                &state,
                &context.session_id,
                &context,
                capture,
                Err(PlatformError::new(
                    PlatformErrorCode::CredentialRecoveryRequired,
                )),
                move || async move {
                    observation_marker.store(true, Ordering::SeqCst);
                    Ok(BoundCredentialObservation::Match)
                },
            ))
            .expect_err("Match cannot override explicit durability failure");
        assert_eq!(error.code, "credential_recovery_required");
        assert!(
            !observation_called.load(Ordering::SeqCst),
            "durability-unknown CAS must settle before and suppress native observation"
        );
        let unknown = shell
            .get_provider_discovery(&context.session_id)
            .expect("load durable unknown outcome");
        assert_eq!(unknown.state, "unknown_outcome");
        assert_eq!(unknown.unknown_operation.as_deref(), Some("atomic_commit"));
        assert!(unknown.active_operation_id.is_none());
        assert_eq!(
            state.discovery_credential_lease_status(&binding),
            CredentialStatus::Missing
        );
        assert!(
            shell
                .list_provider_connections()
                .expect("list provider connections")
                .iter()
                .all(|connection| connection.id != context.connection_id),
            "durability-unknown native bytes are never adopted"
        );
    }

    #[tokio::test]
    async fn product_legacy_selection_carries_the_admission_lease_from_the_raw_read() {
        let root = tempdir().expect("temporary root");
        let provider_profile_id =
            lorepia_shell_api::test_support::seed_synthetic_legacy_provider_profile(root.path())
                .expect("seed legacy provider profile");
        let shell = ShellApi::open_data_root(root.path()).expect("open Shell");
        let state = Arc::new(AppState::new(root.path().to_path_buf()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let reader = FakeLegacyGenerationCredentialReader {
            value: Mutex::new(Some(NativeCredential::new(
                "synthetic-legacy-selection-secret".to_owned(),
            ))),
            calls: Arc::clone(&calls),
        };
        let credential = legacy_credential_for_selection_with_reader(
            &state,
            &shell,
            &provider_profile_id,
            &reader,
        )
        .await
        .expect("read legacy selection with its admission carrier");
        assert_eq!(
            *calls.lock().expect("fake legacy reader calls"),
            vec![provider_profile_id]
        );

        let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
        let (acquired_sender, mut acquired_receiver) = tokio::sync::oneshot::channel();
        let waiter_state = Arc::clone(&state);
        let waiter = tokio::spawn(async move {
            entered_sender.send(()).expect("signal waiter entry");
            let _guard = waiter_state.lock_legacy_credential_admission().await;
            let _ = acquired_sender.send(());
        });
        entered_receiver
            .await
            .expect("legacy mutation waiter entered");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                &mut acquired_receiver,
            )
            .await
            .is_err(),
            "the production selection carrier must retain the legacy lock"
        );
        drop(credential);
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut acquired_receiver)
            .await
            .expect("legacy waiter released")
            .expect("legacy waiter acquired lock");
        waiter.await.expect("legacy waiter task");
    }

    #[tokio::test]
    async fn foreground_generation_carrier_blocks_credential_mutation_until_provider_finishes() {
        let root = tempdir().expect("temporary root");
        let state = Arc::new(AppState::new(root.path().to_path_buf()));
        let dispatch_lease = state.lease_provider_credential_operation().await;
        let credential = lorepia_shell_api::GenerationCredential::connection_with_access_authority_and_dispatch_lease(
            "generation-lease-connection",
            Some(lorepia_shell_api::SecretCredential::new(
                "synthetic-generation-lease-secret",
            )),
            lorepia_shell_api::ProviderCredentialAccessAuthorityContext {
                authority_id: "generation-lease-authority".to_owned(),
                connection_binding_sha256: "a".repeat(64),
            },
            lorepia_shell_api::TaskCredentialLease::new(dispatch_lease),
        );
        let (provider_entered_sender, provider_entered_receiver) = tokio::sync::oneshot::channel();
        let (provider_release_sender, provider_release_receiver) = tokio::sync::oneshot::channel();
        let provider = tokio::spawn(async move {
            provider_entered_sender
                .send(())
                .expect("signal foreground provider entry");
            provider_release_receiver
                .await
                .expect("release foreground provider");
            drop(credential);
        });
        provider_entered_receiver
            .await
            .expect("foreground provider entered");

        let (mutation_entered_sender, mutation_entered_receiver) = tokio::sync::oneshot::channel();
        let (mutation_acquired_sender, mut mutation_acquired_receiver) =
            tokio::sync::oneshot::channel();
        let mutation_state = Arc::clone(&state);
        let mutation = tokio::spawn(async move {
            mutation_entered_sender
                .send(())
                .expect("signal credential mutation entry");
            let _operation = mutation_state.lock_provider_credential_operation().await;
            let _ = mutation_acquired_sender.send(());
        });
        mutation_entered_receiver
            .await
            .expect("credential mutation entered");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                &mut mutation_acquired_receiver,
            )
            .await
            .is_err(),
            "credential replacement/removal must wait while generation owns in-memory A"
        );

        provider_release_sender
            .send(())
            .expect("finish foreground provider");
        provider.await.expect("foreground provider task");
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            &mut mutation_acquired_receiver,
        )
        .await
        .expect("credential mutation released after provider")
        .expect("credential mutation acquired write lease");
        mutation.await.expect("credential mutation task");
    }

    #[test]
    fn prepared_install_with_native_missing_projects_unreadable_not_missing() {
        let root = tempdir().expect("temporary root");
        let shell = ShellApi::open_data_root(root.path()).expect("open Shell");
        create_credential_connection(&shell, "prepared-status");

        assert!(matches!(
            status_only_connection_access(&shell, "prepared-status")
                .expect("classify fresh connection"),
            StatusOnlyConnectionAccess::Unowned
        ));
        assert_eq!(
            status_only_unowned_observation(Ok(CredentialStatus::Missing)),
            CredentialStatus::Missing
        );

        let authority = shell
            .propose_provider_credential_install_authority("prepared-status")
            .expect("propose install authority");
        shell
            .prepare_provider_credential_install_operation(
                "prepared-status",
                &authority,
                ProviderCredentialSlotStatusInput::Missing,
            )
            .expect("durably prepare install");
        let durable_access = status_only_connection_access(&shell, "prepared-status")
            .expect("classify unresolved prepared install");
        let projected = match durable_access {
            StatusOnlyConnectionAccess::Unreadable => CredentialStatus::Unreadable,
            StatusOnlyConnectionAccess::Unowned => {
                status_only_unowned_observation(Ok(CredentialStatus::Missing))
            }
            StatusOnlyConnectionAccess::Settled(_) => {
                panic!("a prepared install cannot expose settled authority")
            }
        };
        assert_eq!(
            projected,
            CredentialStatus::Unreadable,
            "native Missing cannot hide an unresolved durable install"
        );
    }

    #[test]
    fn status_only_platform_errors_are_fail_soft_but_never_available() {
        assert_eq!(
            status_only_unowned_observation(Ok(CredentialStatus::Available)),
            CredentialStatus::Unreadable,
            "a present unowned/orphan slot must not become available"
        );
        let unavailable = PlatformError::new(PlatformErrorCode::StorageUnavailable);
        assert_eq!(
            status_only_unowned_observation(Err(unavailable)),
            CredentialStatus::Unreadable
        );
        assert_eq!(
            status_only_legacy_observation(Err(PlatformError::new(
                PlatformErrorCode::StorageUnavailable,
            ))),
            CredentialStatus::Unreadable
        );
        assert_eq!(
            status_only_legacy_observation(Ok(LegacyCredentialObservation::Bound)),
            CredentialStatus::Unreadable
        );
    }

    #[test]
    fn completed_removal_with_native_missing_returns_to_unowned_missing_state() {
        let root = tempdir().expect("temporary root");
        let shell = ShellApi::open_data_root(root.path()).expect("open Shell");
        create_credential_connection(&shell, "removed-status");
        let install_authority = shell
            .propose_provider_credential_install_authority("removed-status")
            .expect("propose install authority");
        let install = shell
            .prepare_provider_credential_install_operation(
                "removed-status",
                &install_authority,
                ProviderCredentialSlotStatusInput::Missing,
            )
            .expect("prepare install");
        shell
            .start_provider_credential_operation(&install.operation_id, &install.plan_sha256)
            .expect("start install");
        shell
            .finish_provider_credential_operation(
                &install.operation_id,
                &install.plan_sha256,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("finish install");
        assert!(matches!(
            status_only_connection_access(&shell, "removed-status")
                .expect("resolve installed authority"),
            StatusOnlyConnectionAccess::Settled(_)
        ));

        let removal = shell
            .prepare_provider_credential_operation(
                "removed-status",
                ProviderCredentialOperationKindInput::RemoveCredential,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare removal");
        shell
            .start_provider_credential_operation(&removal.operation_id, &removal.plan_sha256)
            .expect("start removal");
        shell
            .finish_provider_credential_operation(
                &removal.operation_id,
                &removal.plan_sha256,
                ProviderCredentialSlotStatusInput::Missing,
            )
            .expect("finish removal");
        assert!(matches!(
            status_only_connection_access(&shell, "removed-status")
                .expect("resolve removed authority"),
            StatusOnlyConnectionAccess::Unowned
        ));
        assert_eq!(
            status_only_unowned_observation(Ok(CredentialStatus::Missing)),
            CredentialStatus::Missing
        );
    }

    fn create_credential_connection(shell: &ShellApi, id: &str) {
        let template = shell
            .list_provider_templates()
            .expect("list templates")
            .into_iter()
            .find(|template| {
                template.credential_required
                    && template.default_network_mode == "public"
                    && template.default_api_origin.is_some()
            })
            .expect("credential-bound public template");
        let origin = template.default_api_origin.expect("template origin");
        shell
            .create_provider_connection(CreateProviderConnectionInput {
                id: id.to_owned(),
                template_id: template.id,
                template_version: template.manifest_version,
                display_name: format!("Synthetic {id}"),
                api_origin: origin.clone(),
                api_base_path: None,
                network_mode: ProviderNetworkModeInput::Public,
                local_network_approval: None,
                values: Vec::new(),
                approved_credential_origin: Some(origin),
                timeout_seconds: 30,
            })
            .expect("create credential-bound connection");
    }
}
