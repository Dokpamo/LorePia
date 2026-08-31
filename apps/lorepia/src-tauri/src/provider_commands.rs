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

const MAXIMUM_PROVIDER_CURL_BYTES: usize = 1024 * 1024;
const MAXIMUM_SIGNED_CATALOG_BYTES: u64 = 4 * 1024 * 1024;

struct ActiveDiscoveryRequest {
    request_id: Uuid,
    cancel: tokio::sync::watch::Sender<bool>,
}

struct ActiveDiscoveryRequestRegistration {
    session_id: String,
    request_id: Uuid,
}

impl Drop for ActiveDiscoveryRequestRegistration {
    fn drop(&mut self) {
        if let Ok(mut requests) = active_discovery_requests().lock()
            && requests
                .get(&self.session_id)
                .is_some_and(|request| request.request_id == self.request_id)
        {
            requests.remove(&self.session_id);
        }
    }
}

fn active_discovery_requests() -> &'static Mutex<HashMap<String, ActiveDiscoveryRequest>> {
    static REQUESTS: OnceLock<Mutex<HashMap<String, ActiveDiscoveryRequest>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_active_discovery_request(
    session_id: &str,
) -> CommandResult<(
    ActiveDiscoveryRequestRegistration,
    tokio::sync::watch::Receiver<bool>,
)> {
    let mut requests = active_discovery_requests()
        .lock()
        .map_err(|_| CommandError::internal())?;
    if requests.contains_key(session_id) {
        return Err(CommandError::busy());
    }
    let request_id = Uuid::new_v4();
    let (cancel, cancelled) = tokio::sync::watch::channel(false);
    requests.insert(
        session_id.to_owned(),
        ActiveDiscoveryRequest { request_id, cancel },
    );
    Ok((
        ActiveDiscoveryRequestRegistration {
            session_id: session_id.to_owned(),
            request_id,
        },
        cancelled,
    ))
}

fn signal_active_discovery_request_cancellation(session_id: &str) -> CommandResult<()> {
    let requests = active_discovery_requests()
        .lock()
        .map_err(|_| CommandError::internal())?;
    if let Some(request) = requests.get(session_id) {
        let _ = request.cancel.send(true);
    }
    Ok(())
}

type DiscoveryVaultFuture<'a, T> = Pin<Box<dyn Future<Output = PlatformResult<T>> + Send + 'a>>;
type ConnectionSlotGuardFuture<'a> = Pin<Box<dyn Future<Output = CommandResult<()>> + Send + 'a>>;
type ExistingConnectionCredentialReadFuture<'a> = Pin<
    Box<
        dyn Future<
                Output = CommandResult<
                    crate::credential_operations::ProviderConnectionCredentialRead,
                >,
            > + Send
            + 'a,
    >,
>;

struct CapturedDiscoveryCredential {
    value: NativeCredential,
    status: NativeCaptureStatus,
}

enum PreparedDiscoveryCredentialStore {
    Platform(PreparedBoundCredentialStore),
    #[cfg(test)]
    Fake {
        reference: String,
        value: NativeCredential,
        authority: CredentialAuthority,
    },
}

/// Rust-only one-use approval carried across the prompt-without-lock boundary.
/// The platform receipt cannot be cloned or serialized; the fake variant is
/// compiled only into this module's tests.
enum DiscoveryCompensationConfirmation {
    Platform(NativeCredentialEffectConfirmation),
    #[cfg(test)]
    Fake {
        effect: NativeCredentialEffect,
        target_id: String,
        origin: String,
        revision: String,
    },
}

impl DiscoveryCompensationConfirmation {
    fn consume_exact(self, context: &NativeCredentialEffectContext) -> PlatformResult<()> {
        match self {
            Self::Platform(confirmation) => confirmation.consume_exact(
                context.effect(),
                context.target_id(),
                context.origin(),
                context.revision(),
            ),
            #[cfg(test)]
            Self::Fake {
                effect,
                target_id,
                origin,
                revision,
            } => {
                if effect == context.effect()
                    && target_id == context.target_id()
                    && origin == context.origin()
                    && revision == context.revision()
                {
                    Ok(())
                } else {
                    Err(tauri_plugin_lorepia_platform::PlatformError::new(
                        PlatformErrorCode::InvalidInput,
                    ))
                }
            }
        }
    }
}

impl PreparedDiscoveryCredentialStore {
    fn into_platform(self) -> PreparedBoundCredentialStore {
        match self {
            Self::Platform(prepared) => prepared,
            #[cfg(test)]
            Self::Fake { .. } => {
                unreachable!("platform vault received a fake prepared credential store")
            }
        }
    }

    #[cfg(test)]
    fn into_fake(self) -> (String, NativeCredential, CredentialAuthority) {
        match self {
            Self::Fake {
                reference,
                value,
                authority,
            } => (reference, value, authority),
            Self::Platform(_) => {
                unreachable!("fake vault received a platform prepared credential store")
            }
        }
    }
}

trait DiscoveryCredentialVault: Send + Sync {
    fn status_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> DiscoveryVaultFuture<'a, CredentialStatus>;

    fn capture_bound(&self) -> DiscoveryVaultFuture<'_, CapturedDiscoveryCredential>;

    fn prepare_bound_store(
        &self,
        reference: &str,
        value: NativeCredential,
        authority: &CredentialAuthority,
    ) -> PlatformResult<PreparedDiscoveryCredentialStore>;

    fn store_prepared(
        &self,
        prepared: PreparedDiscoveryCredentialStore,
    ) -> DiscoveryVaultFuture<'_, ()>;

    fn observe_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> DiscoveryVaultFuture<'a, BoundCredentialObservation>;

    fn delete_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> DiscoveryVaultFuture<'a, ()>;

    fn confirm_compensation(
        &self,
        context: NativeCredentialEffectContext,
    ) -> DiscoveryVaultFuture<'_, DiscoveryCompensationConfirmation>;
}

struct PlatformDiscoveryCredentialVault<'a> {
    app: &'a AppHandle,
}

impl DiscoveryCredentialVault for PlatformDiscoveryCredentialVault<'_> {
    fn status_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> DiscoveryVaultFuture<'a, CredentialStatus> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .bound_credential_status(reference, &authority)
                .await
        })
    }

    fn capture_bound(&self) -> DiscoveryVaultFuture<'_, CapturedDiscoveryCredential> {
        Box::pin(async move {
            let captured = self
                .app
                .lorepia_platform()
                .capture_credential_text_from_clipboard()
                .await?;
            Ok(CapturedDiscoveryCredential {
                status: captured.status(),
                value: NativeCredential::new(captured.into_secret_string()),
            })
        })
    }

    fn prepare_bound_store(
        &self,
        reference: &str,
        value: NativeCredential,
        authority: &CredentialAuthority,
    ) -> PlatformResult<PreparedDiscoveryCredentialStore> {
        self.app
            .lorepia_platform()
            .prepare_bound_credential_store(reference, value, authority)
            .map(PreparedDiscoveryCredentialStore::Platform)
    }

    fn store_prepared(
        &self,
        prepared: PreparedDiscoveryCredentialStore,
    ) -> DiscoveryVaultFuture<'_, ()> {
        let prepared = prepared.into_platform();
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .store_prepared_bound_credential(prepared)
                .await
        })
    }

    fn observe_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> DiscoveryVaultFuture<'a, BoundCredentialObservation> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .observe_bound_credential(reference, &authority)
                .await
        })
    }

    fn delete_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> DiscoveryVaultFuture<'a, ()> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .delete_bound_credential(reference, &authority)
                .await
        })
    }

    fn confirm_compensation(
        &self,
        context: NativeCredentialEffectContext,
    ) -> DiscoveryVaultFuture<'_, DiscoveryCompensationConfirmation> {
        Box::pin(async move {
            let confirmation = self
                .app
                .lorepia_platform()
                .confirm_credential_effect(context)
                .await?;
            Ok(DiscoveryCompensationConfirmation::Platform(confirmation))
        })
    }
}

trait NewConnectionSlotGuard: Send + Sync {
    fn ensure_missing<'a>(&'a self, connection_id: &'a str) -> ConnectionSlotGuardFuture<'a>;
}

struct PlatformNewConnectionSlotGuard<'a> {
    app: &'a AppHandle,
}

impl NewConnectionSlotGuard for PlatformNewConnectionSlotGuard<'_> {
    fn ensure_missing<'a>(&'a self, connection_id: &'a str) -> ConnectionSlotGuardFuture<'a> {
        Box::pin(async move {
            crate::credential_operations::ensure_new_connection_slot_missing(
                self.app,
                connection_id,
            )
            .await
        })
    }
}

trait ExistingConnectionCredentialReader: Send + Sync {
    fn read<'a>(
        &'a self,
        shell: &'a shell::ShellApi,
        connection_id: &'a str,
    ) -> ExistingConnectionCredentialReadFuture<'a>;
}

struct PlatformExistingConnectionCredentialReader<'a> {
    app: &'a AppHandle,
}

impl ExistingConnectionCredentialReader for PlatformExistingConnectionCredentialReader<'_> {
    fn read<'a>(
        &'a self,
        shell: &'a shell::ShellApi,
        connection_id: &'a str,
    ) -> ExistingConnectionCredentialReadFuture<'a> {
        Box::pin(async move {
            crate::credential_operations::read_provider_connection_credential(
                self.app,
                shell,
                connection_id,
            )
            .await
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
enum CredentialInstallRecoveryAction {
    DeferToCore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialCompensationDeleteOutcome {
    Complete,
    Fail,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompensationObserveErrorPolicy {
    Propagate,
    Defer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompensationCredentialEffectPolicy {
    RequireNativeConfirmation,
    ObserveOnly,
}

#[derive(Debug)]
enum DiscoveryCompensationDriveResult {
    Finished(shell::ProviderDiscoverySessionDto),
    NativeConfirmationRequired {
        session: shell::ProviderDiscoverySessionDto,
        context: NativeCredentialEffectContext,
    },
}

#[derive(Clone)]
struct DiscoveryCredentialCommitCandidate {
    session_id: String,
    session_revision: u64,
    connection_id: String,
    commit_attempt_id: String,
    commit_plan_sha256: String,
}

trait DiscoveryCredentialInstallJournal: Send + Sync {
    fn install_context(
        &self,
        session_id: &str,
    ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto>;

    fn reserve_install(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
    ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto>;

    fn start_install(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_reservation_id: &str,
    ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto>;

    fn attest_no_effect(
        &self,
        session_id: &str,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_id: &str,
    ) -> CommandResult<()>;

    #[allow(clippy::too_many_arguments)]
    fn mark_durability_unknown(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_id: &str,
        connection_id: &str,
        connection_binding_sha256: &str,
    ) -> CommandResult<()>;
}

impl DiscoveryCredentialInstallJournal for shell::ShellApi {
    fn install_context(
        &self,
        session_id: &str,
    ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto> {
        self.get_provider_discovery_credential_install_context(session_id)
            .map_err(Into::into)
    }

    fn start_install(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_reservation_id: &str,
    ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto> {
        self.start_provider_discovery_credential_install(
            session_id,
            expected_revision,
            operation_id,
            commit_attempt_id,
            commit_plan_sha256,
            native_execution_reservation_id,
        )
        .map_err(Into::into)
    }

    fn reserve_install(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
    ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto> {
        self.reserve_provider_discovery_credential_install(
            session_id,
            expected_revision,
            operation_id,
            commit_attempt_id,
            commit_plan_sha256,
        )
        .map_err(Into::into)
    }

    fn attest_no_effect(
        &self,
        session_id: &str,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_id: &str,
    ) -> CommandResult<()> {
        self.attest_provider_discovery_credential_install_no_effect(
            session_id,
            operation_id,
            commit_attempt_id,
            commit_plan_sha256,
            native_execution_id,
        )
        .map(|_| ())
        .map_err(Into::into)
    }

    fn mark_durability_unknown(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_id: &str,
        connection_id: &str,
        connection_binding_sha256: &str,
    ) -> CommandResult<()> {
        self.mark_provider_discovery_credential_install_durability_unknown(
            session_id,
            expected_revision,
            operation_id,
            commit_attempt_id,
            commit_plan_sha256,
            native_execution_id,
            connection_id,
            connection_binding_sha256,
        )
        .map(|_| ())
        .map_err(Into::into)
    }
}

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
pub struct CreateProviderConnectionRequest {
    pub input: shell::CreateProviderConnectionInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProviderConnectionRequest {
    pub input: shell::UpdateProviderConnectionInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConnectionRequest {
    pub connection_id: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartProviderModelSyncRequest {
    pub connection_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncJobRequest {
    pub job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListProviderModelSyncsRequest {
    pub connection_id: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveProviderModelSyncRequest {
    pub job_id: String,
    pub review_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PollProviderModelSyncEventsRequest {
    pub job_id: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckProviderModelSyncEventRequest {
    pub job_id: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeginProviderDiscoveryRequest {
    pub input: shell::BeginProviderDiscoveryInput,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeginProviderDiscoveryCurlRequest {
    pub input: shell::BeginProviderDiscoveryCurlInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitRequest {
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PollProviderDiscoveryEventsForSessionRequest {
    pub session_id: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoverySessionRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordProviderDiscoveryAssistantFailureRequest {
    pub session_id: String,
    pub kind: shell::DiscoveryAssistantFailureKindInput,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterruptProviderDiscoveryAssistantRequest {
    pub session_id: String,
    pub outcome: shell::DiscoveryAssistantInterruptionOutcomeInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinueProviderDiscoveryRequest {
    pub input: shell::ContinueProviderDiscoveryInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupplyProviderDiscoveryDocumentEvidenceRequest {
    pub session_id: String,
    pub expected_revision: u64,
    pub document_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupplyProviderDiscoveryCurlEvidenceRequest {
    pub session_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelProviderDiscoveryRequest {
    pub session_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitProviderDiscoveryRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CapturedProviderDiscoveryDto {
    pub session: shell::ProviderDiscoverySessionDto,
    pub capture: NativeCaptureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoveryEventRequest {
    pub event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryCompensationStepsRequest {
    pub commit_attempt_id: String,
}

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
pub fn list_provider_connections(
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::ProviderConnectionDto>> {
    state
        .shell()?
        .list_provider_connections()
        .map_err(Into::into)
}

#[tauri::command]
pub async fn create_provider_connection(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CreateProviderConnectionRequest,
) -> CommandResult<shell::ProviderConnectionDto> {
    let shell = state.shell()?;
    let _operation = state.lock_provider_credential_operation().await;
    let CreateProviderConnectionRequest { input } = request;
    create_provider_connection_with_slot_guard(
        &shell,
        input,
        &PlatformNewConnectionSlotGuard { app: &app },
    )
    .await
}

async fn create_provider_connection_with_slot_guard(
    shell: &shell::ShellApi,
    input: shell::CreateProviderConnectionInput,
    slot_guard: &dyn NewConnectionSlotGuard,
) -> CommandResult<shell::ProviderConnectionDto> {
    let connection_id = input.id.clone();
    if shell
        .list_provider_connections()?
        .iter()
        .any(|connection| connection.id == connection_id)
    {
        return Err(CommandError::invalid_input());
    }
    let template = shell
        .list_provider_templates()?
        .into_iter()
        .find(|template| {
            template.id == input.template_id && template.manifest_version == input.template_version
        })
        .ok_or_else(CommandError::invalid_input)?;
    let credential_binding_expected =
        template.credential_required || input.approved_credential_origin.is_some();
    if credential_binding_expected {
        slot_guard.ensure_missing(&connection_id).await?;
    }

    let connection = shell.create_provider_connection(input)?;
    if connection.id != connection_id {
        return Err(CommandError::internal());
    }
    if connection.credential_binding_required != credential_binding_expected {
        return Err(CommandError::internal());
    }
    Ok(connection)
}

#[tauri::command]
pub fn upsert_provider_connection(
    state: State<'_, AppState>,
    request: UpdateProviderConnectionRequest,
) -> CommandResult<shell::ProviderConnectionDto> {
    state
        .shell()?
        .upsert_provider_connection(request.input)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn delete_provider_connection(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ProviderConnectionRequest,
) -> CommandResult<()> {
    let shell = state.shell()?;
    let connection = find_connection(&shell, &request.connection_id)?;
    let legacy_raw =
        shell.provider_connection_uses_legacy_raw_credential(&request.connection_id)?;
    let confirmation = if connection.credential_binding_required {
        let confirmation = if legacy_raw {
            crate::commands::confirm_legacy_credential_effect(
                &app,
                &shell,
                &request.connection_id,
                NativeCredentialEffect::Archive,
            )
            .await?
        } else {
            let context =
                crate::credential_operations::provider_connection_credential_effect_context(
                    &shell,
                    &request.connection_id,
                    NativeCredentialEffect::Archive,
                )?;
            let confirmation = app
                .lorepia_platform()
                .confirm_credential_effect(context)
                .await?;
            let latest =
                crate::credential_operations::provider_connection_credential_effect_context(
                    &shell,
                    &request.connection_id,
                    NativeCredentialEffect::Archive,
                )?;
            if confirmation.context() != &latest {
                return Err(CommandError::invalid_input());
            }
            confirmation
        };
        if find_connection(&shell, &request.connection_id)? != connection
            || shell.provider_connection_uses_legacy_raw_credential(&request.connection_id)?
                != legacy_raw
        {
            return Err(CommandError::invalid_input());
        }
        Some(confirmation)
    } else {
        None
    };
    // Never let a renderer-triggered native modal hold either global
    // credential lock. Reacquire only after the one-use approval exists.
    let _legacy_archive_operation = if legacy_raw {
        Some(state.lock_legacy_provider_credential_archive().await)
    } else {
        None
    };
    let _operation = if legacy_raw {
        None
    } else {
        Some(state.lock_provider_credential_operation().await)
    };
    if find_connection(&shell, &request.connection_id)? != connection
        || shell.provider_connection_uses_legacy_raw_credential(&request.connection_id)?
            != legacy_raw
    {
        return Err(CommandError::invalid_input());
    }
    crate::credential_operations::archive_provider_connection(
        &app,
        &shell,
        &request.connection_id,
        connection.credential_binding_required,
        legacy_raw,
        confirmation,
    )
    .await
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

#[tauri::command]
pub async fn start_provider_model_sync(
    app: AppHandle,
    state: State<'_, AppState>,
    request: StartProviderModelSyncRequest,
) -> CommandResult<shell::ModelSyncStartedDto> {
    let shell = state.shell()?;
    let dispatch_lease = state.lease_provider_credential_operation().await;
    start_provider_model_sync_with_reader(
        &shell,
        &request.connection_id,
        &PlatformExistingConnectionCredentialReader { app: &app },
        Some(shell::TaskCredentialLease::new(dispatch_lease)),
    )
    .await
}

#[tauri::command]
pub fn get_provider_model_sync(
    state: State<'_, AppState>,
    request: ModelSyncJobRequest,
) -> CommandResult<shell::ModelSyncJobDto> {
    state
        .shell()?
        .get_provider_model_sync(&request.job_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_provider_model_syncs(
    state: State<'_, AppState>,
    request: ListProviderModelSyncsRequest,
) -> CommandResult<Vec<shell::ModelSyncJobDto>> {
    state
        .shell()?
        .list_provider_model_syncs(&request.connection_id, request.limit)
        .map_err(Into::into)
}

#[tauri::command]
pub fn approve_provider_model_sync(
    state: State<'_, AppState>,
    request: ApproveProviderModelSyncRequest,
) -> CommandResult<shell::ModelSyncJobDto> {
    state
        .shell()?
        .approve_provider_model_sync(&request.job_id, &request.review_sha256)
        .map_err(Into::into)
}

#[tauri::command]
pub fn cancel_provider_model_sync(
    state: State<'_, AppState>,
    request: ModelSyncJobRequest,
) -> CommandResult<shell::ModelSyncJobDto> {
    state
        .shell()?
        .cancel_provider_model_sync(&request.job_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn poll_provider_model_sync_events(
    state: State<'_, AppState>,
    request: PollProviderModelSyncEventsRequest,
) -> CommandResult<Vec<shell::ModelSyncEventDto>> {
    state
        .shell()?
        .poll_provider_model_sync_events(&request.job_id, request.limit)
        .map_err(Into::into)
}

#[tauri::command]
pub fn ack_provider_model_sync_event(
    state: State<'_, AppState>,
    request: AckProviderModelSyncEventRequest,
) -> CommandResult<bool> {
    state
        .shell()?
        .ack_provider_model_sync_event(&request.job_id, request.sequence)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn begin_provider_discovery(
    app: AppHandle,
    state: State<'_, AppState>,
    request: BeginProviderDiscoveryRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = state.shell()?;
    begin_provider_discovery_with_reader(
        &shell,
        request.input,
        &PlatformExistingConnectionCredentialReader { app: &app },
    )
    .await
}

#[tauri::command]
pub async fn begin_provider_discovery_curl(
    app: AppHandle,
    state: State<'_, AppState>,
    request: BeginProviderDiscoveryCurlRequest,
) -> CommandResult<CapturedProviderDiscoveryDto> {
    state.ensure_ready()?;
    let captured = app
        .lorepia_platform()
        .capture_sensitive_text_from_clipboard(MAXIMUM_PROVIDER_CURL_BYTES)
        .await?;
    let capture = captured.status();
    let curl = bounded_secret_curl(captured.into_secret_string())?;
    let shell = state.shell()?;
    let session = begin_provider_discovery_curl_with_reader(
        &shell,
        request.input,
        curl,
        &PlatformExistingConnectionCredentialReader { app: &app },
    )
    .await?;
    Ok(CapturedProviderDiscoveryDto { session, capture })
}

#[tauri::command]
pub fn list_provider_discoveries(
    state: State<'_, AppState>,
    request: LimitRequest,
) -> CommandResult<Vec<shell::ProviderDiscoverySessionDto>> {
    state
        .shell()?
        .list_provider_discoveries(request.limit)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_provider_discovery(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .get_provider_discovery(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_provider_discovery_candidates(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Vec<shell::DiscoveryCandidateDto>> {
    state
        .shell()?
        .list_provider_discovery_candidates(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_provider_discovery_evidence(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Vec<shell::DiscoveryEvidenceDto>> {
    state
        .shell()?
        .list_provider_discovery_evidence(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_provider_discovery_approvals(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Vec<shell::DiscoveryApprovalRecordDto>> {
    state
        .shell()?
        .list_provider_discovery_approvals(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_provider_discovery_review(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Option<shell::DiscoveryReviewDto>> {
    state
        .shell()?
        .get_provider_discovery_review(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_provider_discovery_approval_proposal(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Option<shell::ProviderDiscoveryApprovalProposalDto>> {
    state
        .shell()?
        .get_provider_discovery_approval_proposal(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_provider_discovery_review_proposal(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Option<shell::ProviderDiscoveryReviewProposalDto>> {
    state
        .shell()?
        .get_provider_discovery_review_proposal(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_provider_discovery_assistant_resume_boundary(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Option<shell::DiscoveryAssistantResumeBoundaryDto>> {
    state
        .shell()?
        .get_provider_discovery_assistant_resume_boundary(&request.session_id)
        .map_err(Into::into)
}

/// Remote setup-assistant execution stays unavailable until Rust can price and
/// tokenize the exact prepared provider request. Deliberately accepting neither
/// application state nor a platform handle makes credential access and provider
/// construction impossible on this command path.
#[tauri::command]
pub fn run_provider_discovery_assistant_turn(
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::DiscoveryAssistantHostActionDto> {
    let _ = request;
    Err(CommandError::assistant_pricing_unavailable())
}

#[tauri::command]
pub fn resume_provider_discovery_assistant_core_host_action(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .resume_provider_discovery_assistant_core_host_action(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn approve_provider_discovery_assistant_retry(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .approve_provider_discovery_assistant_retry(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn request_provider_discovery_assistant_revision(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .request_provider_discovery_assistant_revision(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn accept_provider_discovery_assistant_draft(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .accept_provider_discovery_assistant_draft(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn record_provider_discovery_assistant_failure(
    state: State<'_, AppState>,
    request: RecordProviderDiscoveryAssistantFailureRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .record_provider_discovery_assistant_failure(
            &request.session_id,
            request.kind,
            request.retryable,
        )
        .map_err(Into::into)
}

#[tauri::command]
pub fn interrupt_provider_discovery_assistant(
    state: State<'_, AppState>,
    request: InterruptProviderDiscoveryAssistantRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .interrupt_provider_discovery_assistant(&request.session_id, request.outcome)
        .map_err(Into::into)
}

#[tauri::command]
pub fn restart_provider_discovery_assistant_after_interruption(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .restart_provider_discovery_assistant_after_interruption(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn continue_provider_discovery(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ContinueProviderDiscoveryRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = state.shell()?;
    let input = request.input;
    // Keep the repository-wide provider credential read lease for the entire
    // dispatch, not merely while copying the secret. A native replacement or
    // removal therefore cannot race an authenticated discovery request.
    let dispatch_lease = state.lease_provider_credential_operation().await;
    let session = shell.get_provider_discovery(&input.session_id)?;
    let credential = credential_for_discovery_action(
        &state,
        &shell,
        &session,
        input.expected_revision,
        &input.action,
    )?;
    let mut next = if let Some(credential) = credential {
        let (registration, cancelled) = register_active_discovery_request(&input.session_id)?;
        continue_provider_discovery_with_credential_dispatch_off_runtime(
            &shell,
            input,
            credential,
            shell::TaskCredentialLease::new(dispatch_lease),
            cancelled,
            registration,
        )
        .await?
    } else {
        drop(dispatch_lease);
        continue_provider_discovery_off_runtime(&shell, input, None).await?
    };
    if next.state == "committing" {
        let _operation = state.lock_provider_credential_operation().await;
        let latest = shell.get_provider_discovery(&next.id)?;
        if latest.state == "committing"
            && !latest.cancellation_pending
            && latest.revision == next.revision
        {
            let _ = promote_discovery_credential_lease(&app, &state, &shell, &latest).await?;
        } else {
            next = latest;
        }
    }
    if next.cancellation_pending
        || matches!(
            next.state.as_str(),
            "ready" | "failed" | "cancelled" | "compensating"
        )
    {
        state.clear_discovery_credential_lease(&next.id);
    }
    Ok(next)
}

#[tauri::command]
pub fn supply_provider_discovery_document_evidence(
    state: State<'_, AppState>,
    request: SupplyProviderDiscoveryDocumentEvidenceRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .supply_provider_discovery_document_evidence(
            &request.session_id,
            request.expected_revision,
            &request.document_url,
        )
        .map_err(Into::into)
}

#[tauri::command]
pub async fn supply_provider_discovery_curl_evidence(
    app: AppHandle,
    state: State<'_, AppState>,
    request: SupplyProviderDiscoveryCurlEvidenceRequest,
) -> CommandResult<CapturedProviderDiscoveryDto> {
    state.ensure_ready()?;
    let captured = app
        .lorepia_platform()
        .capture_sensitive_text_from_clipboard(MAXIMUM_PROVIDER_CURL_BYTES)
        .await?;
    let capture = captured.status();
    let curl = bounded_secret_curl(captured.into_secret_string())?;
    let shell = state.shell()?;
    let session = supply_provider_discovery_curl_evidence_off_runtime(
        &shell,
        request.session_id,
        request.expected_revision,
        curl,
    )
    .await?;
    Ok(CapturedProviderDiscoveryDto { session, capture })
}

#[tauri::command]
pub async fn cancel_provider_discovery(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CancelProviderDiscoveryRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = state.shell()?;
    let cancelled = request_provider_discovery_cancellation(&state, &shell, &request)?;
    if cancelled.state != "committing" || !cancelled.cancellation_pending {
        return Ok(cancelled);
    }

    // Only credential compensation remains under the global writer. The
    // durable cancellation request above stays prompt even while another task
    // is resolving or reading provider model pages.
    let latest = {
        let _operation = state.lock_provider_credential_operation().await;
        let latest = shell.get_provider_discovery(&request.session_id)?;
        if latest.state != "committing" || !latest.cancellation_pending {
            return Ok(latest);
        }

        // A discovery credential install marks the atomic operation started before
        // the native vault write. Route cancellation back through Core's existing
        // commit-cancellation transition so its durable compensation recipe owns
        // removal of the slot; never delete an unjournaled reference here.
        let _ = shell.commit_provider_discovery(&request.session_id, None);
        shell.get_provider_discovery(&request.session_id)?
    };
    if latest.state == "compensating" {
        return drive_provider_discovery_compensation_explicit(
            &app,
            &state,
            &shell,
            latest,
            false,
            CompensationObserveErrorPolicy::Propagate,
        )
        .await;
    }
    Ok(latest)
}

fn request_provider_discovery_cancellation(
    state: &AppState,
    shell: &shell::ShellApi,
    request: &CancelProviderDiscoveryRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let current = shell.get_provider_discovery(&request.session_id)?;
    if current.revision != request.expected_revision {
        return shell
            .cancel_provider_discovery(&request.session_id, request.expected_revision)
            .map_err(Into::into);
    }
    // Revoke the backend-owned dispatch token before the durable cancellation
    // transition can be accepted. The active registration remains owned by
    // the blocking worker even if the invoking async task is dropped.
    signal_active_discovery_request_cancellation(&request.session_id)?;
    let cancelled =
        shell.cancel_provider_discovery(&request.session_id, request.expected_revision)?;
    state.clear_discovery_credential_lease(&request.session_id);
    Ok(cancelled)
}

#[tauri::command]
pub async fn commit_provider_discovery(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CommitProviderDiscoveryRequest,
) -> CommandResult<shell::ProviderConnectionDto> {
    let shell = state.shell()?;
    let CommitProviderDiscoveryRequest { session_id } = request;
    let (error, latest) = {
        let _operation = state.lock_provider_credential_operation().await;
        let session = shell.get_provider_discovery(&session_id)?;
        let _ = promote_discovery_credential_lease(&app, &state, &shell, &session).await?;
        let credential_confirmation = if session.credential_binding_requested {
            let context = shell.get_provider_discovery_credential_install_context(&session_id)?;
            require_started_discovery_credential_install(&context)?;
            let observation = app
                .lorepia_platform()
                .observe_bound_credential(
                    &context.connection_id,
                    &discovery_credential_authority(&context)?,
                )
                .await?;
            if observation != BoundCredentialObservation::Match {
                return Err(CommandError::invalid_input());
            }
            Some(shell::ProviderDiscoveryCredentialCommitConfirmationDto::try_from(&context)?)
        } else {
            None
        };

        match shell.commit_provider_discovery(&session_id, credential_confirmation.as_ref()) {
            Ok(connection) => {
                state.clear_discovery_credential_lease(&session_id);
                return Ok(connection);
            }
            Err(error) => (error, shell.get_provider_discovery(&session_id).ok()),
        }
    };
    match latest {
        Some(latest) if latest.state == "compensating" => {
            state.clear_discovery_credential_lease(&session_id);
            drive_provider_discovery_compensation_explicit(
                &app,
                &state,
                &shell,
                latest,
                false,
                CompensationObserveErrorPolicy::Propagate,
            )
            .await?;
        }
        Some(latest) if latest.state == "ready" => {
            state.clear_discovery_credential_lease(&session_id);
        }
        Some(latest) if latest.state == "unknown_outcome" => {}
        _ => {}
    }
    Err(error.into())
}

fn require_started_discovery_credential_install(
    context: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<()> {
    if context.operation_status == "started"
        && context.native_execution_id.is_some()
        && context.native_execution_reservation_id.as_deref()
            == context.native_execution_id.as_deref()
    {
        return Ok(());
    }
    // A matching envelope from a future database generation is still an
    // orphan until this exact rollback-visible WAL entry proves the native
    // effect was started. Never adopt it from Prepared.
    Err(CommandError::invalid_input())
}

#[tauri::command]
pub fn poll_provider_discovery_events(
    state: State<'_, AppState>,
    request: LimitRequest,
) -> CommandResult<Vec<shell::DiscoveryOutboxEventDto>> {
    state
        .shell()?
        .poll_provider_discovery_events(request.limit)
        .map_err(Into::into)
}

#[tauri::command]
pub fn poll_provider_discovery_events_for_session(
    state: State<'_, AppState>,
    request: PollProviderDiscoveryEventsForSessionRequest,
) -> CommandResult<Vec<shell::DiscoveryOutboxEventDto>> {
    state
        .shell()?
        .poll_provider_discovery_events_for_session(&request.session_id, request.limit)
        .map_err(Into::into)
}

#[tauri::command]
pub fn ack_provider_discovery_event(
    state: State<'_, AppState>,
    request: ProviderDiscoveryEventRequest,
) -> CommandResult<bool> {
    state
        .shell()?
        .ack_provider_discovery_event(&request.event_id)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn recover_provider_discovery(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::DiscoveryRecoveryResultDto>> {
    recover_provider_discovery_backend(&app, &state).await
}

pub(crate) async fn recover_provider_discovery_backend(
    app: &AppHandle,
    state: &AppState,
) -> CommandResult<Vec<shell::DiscoveryRecoveryResultDto>> {
    let _operation = state.lock_provider_credential_operation().await;
    let shell = state.shell()?;
    recover_provider_discovery_with_shell(app, &shell).await
}

pub(crate) async fn recover_provider_discovery_with_shell(
    app: &AppHandle,
    shell: &shell::ShellApi,
) -> CommandResult<Vec<shell::DiscoveryRecoveryResultDto>> {
    let recovered = recover_provider_discovery_credential_installs(shell)?;

    for session in shell.list_unfinished_provider_discovery_recovery_candidates()? {
        if session.state == "compensating" {
            drive_provider_discovery_compensation_observe_only(
                app,
                shell,
                session,
                false,
                CompensationObserveErrorPolicy::Defer,
            )
            .await?;
        }
    }
    Ok(recovered)
}

fn recover_provider_discovery_credential_installs(
    shell: &shell::ShellApi,
) -> CommandResult<Vec<shell::DiscoveryRecoveryResultDto>> {
    let mut recovered = Vec::new();
    let mut previous_candidate_ids = None;
    loop {
        let sessions = shell.list_provider_discovery_credential_recovery_candidates()?;
        if sessions.is_empty() {
            recovered.extend(shell.recover_provider_discovery()?);
            break;
        }
        let candidate_ids = sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        if previous_candidate_ids.as_ref() == Some(&candidate_ids) {
            return Err(CommandError::internal());
        }
        previous_candidate_ids = Some(candidate_ids);

        for session in sessions {
            // Startup always uses the recovery-only projection. It alone can
            // represent a Storage-proven sealed pre37 Started operation with
            // no physical execution ID, which must defer without vault access.
            // Normal product/confirmation paths continue to reject that shape.
            let context =
                shell.get_provider_discovery_credential_install_recovery_context(&session.id)?;
            if context.session_id != session.id
                || context.session_revision != session.revision
                || session.commit_attempt_id.as_deref() != Some(context.commit_attempt_id.as_str())
                || session.commit_plan_sha256.as_deref()
                    != Some(context.commit_plan_sha256.as_str())
                || context.commit_phase != "prepared"
                || context.connection_id != session.connection_id
            {
                return Err(CommandError::internal());
            }
            if context.operation_status == "started" {
                settle_started_discovery_credential_recovery(shell, &session, &context)?;
            } else if context.operation_status != "prepared" {
                return Err(CommandError::internal());
            }
        }

        recovered.extend(shell.recover_provider_discovery()?);
        if shell
            .list_provider_discovery_credential_recovery_candidates()?
            .is_empty()
        {
            break;
        }
    }

    Ok(recovered)
}

fn settle_started_discovery_credential_recovery(
    shell: &shell::ShellApi,
    session: &shell::ProviderDiscoverySessionDto,
    context: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<()> {
    match (
        context.native_execution_reservation_id.as_deref(),
        context.native_execution_id.as_deref(),
    ) {
        (Some(reservation_id), Some(execution_id)) if reservation_id == execution_id => {
            shell.mark_provider_discovery_credential_install_durability_unknown(
                &session.id,
                context.session_revision,
                &context.operation_id,
                &context.commit_attempt_id,
                &context.commit_plan_sha256,
                execution_id,
                &context.connection_id,
                &context.connection_binding_sha256,
            )?;
            Ok(())
        }
        (None, None) => {
            // Sealed pre-37 Started rows intentionally have no physical
            // execution authority. Generic Core recovery classifies them
            // Unknown without native access.
            Ok(())
        }
        _ => Err(CommandError::internal()),
    }
}

#[cfg(test)]
fn credential_install_recovery_action(
    _cancellation_pending: bool,
    operation_status: &str,
    _credential_status: CredentialStatus,
) -> CommandResult<CredentialInstallRecoveryAction> {
    match operation_status {
        "prepared" | "started" => Ok(CredentialInstallRecoveryAction::DeferToCore),
        _ => Err(CommandError::internal()),
    }
}

#[tauri::command]
pub fn list_provider_discovery_compensation_steps(
    state: State<'_, AppState>,
    request: DiscoveryCompensationStepsRequest,
) -> CommandResult<Vec<shell::DiscoveryCompensationRecordDto>> {
    state
        .shell()?
        .list_provider_discovery_compensation_steps(&request.commit_attempt_id)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn continue_provider_discovery_compensation(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = state.shell()?;
    let session = {
        let _operation = state.lock_provider_credential_operation().await;
        shell
            .continue_provider_discovery_compensation(&request.session_id)
            .map_err(CommandError::from)?
    };
    drive_provider_discovery_compensation_explicit(
        &app,
        &state,
        &shell,
        session,
        false,
        CompensationObserveErrorPolicy::Propagate,
    )
    .await
}

#[tauri::command]
pub async fn resume_provider_discovery_compensation(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = state.shell()?;
    let session = {
        let _operation = state.lock_provider_credential_operation().await;
        shell
            .resume_provider_discovery_compensation(&request.session_id)
            .map_err(CommandError::from)?
    };
    drive_provider_discovery_compensation_explicit(
        &app,
        &state,
        &shell,
        session,
        true,
        CompensationObserveErrorPolicy::Propagate,
    )
    .await
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

async fn credential_for_connection_with_reader<R: ExistingConnectionCredentialReader + ?Sized>(
    reader: &R,
    shell: &shell::ShellApi,
    connection_id: &str,
) -> CommandResult<(
    Option<shell::SecretCredential>,
    Option<shell::ProviderCredentialAccessAuthorityContext>,
)> {
    let connection = find_connection(shell, connection_id)?;
    if !connection.credential_binding_required {
        return Ok((None, None));
    }
    let read = reader.read(shell, connection_id).await?;
    Ok((
        native_credential_to_shell(read.credential),
        Some(read.access_authority),
    ))
}

async fn credential_authority_for_existing_connection_with_reader<
    R: ExistingConnectionCredentialReader + ?Sized,
>(
    reader: &R,
    shell: &shell::ShellApi,
    connection_id: &str,
) -> CommandResult<Option<shell::ProviderCredentialAccessAuthorityContext>> {
    if !shell
        .list_provider_connections()?
        .iter()
        .any(|connection| connection.id == connection_id)
    {
        return Ok(None);
    }
    let (credential, access_authority) =
        credential_for_connection_with_reader(reader, shell, connection_id).await?;
    drop(credential);
    Ok(access_authority)
}

async fn start_provider_model_sync_with_reader<R: ExistingConnectionCredentialReader + ?Sized>(
    shell: &shell::ShellApi,
    connection_id: &str,
    reader: &R,
    dispatch_lease: Option<shell::TaskCredentialLease>,
) -> CommandResult<shell::ModelSyncStartedDto> {
    let (credential, access_authority) =
        credential_for_connection_with_reader(reader, shell, connection_id).await?;
    match dispatch_lease {
        Some(dispatch_lease) => shell
            .start_provider_model_sync_with_credential_authority_and_dispatch_lease(
                connection_id,
                credential,
                access_authority,
                dispatch_lease,
            ),
        None => shell.start_provider_model_sync_with_credential_authority(
            connection_id,
            credential,
            access_authority,
        ),
    }
    .map_err(Into::into)
}

async fn run_shell_discovery_off_runtime<T, F>(operation: F) -> CommandResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> shell::ShellResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| CommandError::internal())?
        .map_err(CommandError::from)
}

async fn begin_provider_discovery_with_reader<R: ExistingConnectionCredentialReader + ?Sized>(
    shell: &shell::ShellApi,
    input: shell::BeginProviderDiscoveryInput,
    reader: &R,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let access_authority = credential_authority_for_existing_connection_with_reader(
        reader,
        shell,
        &input.connection_id,
    )
    .await?;
    let shell = shell.clone();
    run_shell_discovery_off_runtime(move || {
        shell.begin_provider_discovery_with_credential_authority(input, access_authority)
    })
    .await
}

async fn begin_provider_discovery_curl_with_reader<
    R: ExistingConnectionCredentialReader + ?Sized,
>(
    shell: &shell::ShellApi,
    input: shell::BeginProviderDiscoveryCurlInput,
    curl: shell::SecretProviderCurl,
    reader: &R,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let access_authority = credential_authority_for_existing_connection_with_reader(
        reader,
        shell,
        &input.connection_id,
    )
    .await?;
    let shell = shell.clone();
    run_shell_discovery_off_runtime(move || {
        shell.begin_provider_discovery_curl_with_credential_authority(input, curl, access_authority)
    })
    .await
}

async fn continue_provider_discovery_off_runtime(
    shell: &shell::ShellApi,
    input: shell::ContinueProviderDiscoveryInput,
    credential: Option<shell::SecretCredential>,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = shell.clone();
    run_shell_discovery_off_runtime(move || shell.continue_provider_discovery(input, credential))
        .await
}

async fn continue_provider_discovery_with_credential_dispatch_off_runtime(
    shell: &shell::ShellApi,
    input: shell::ContinueProviderDiscoveryInput,
    credential: shell::SecretCredential,
    dispatch_lease: shell::TaskCredentialLease,
    cancelled: tokio::sync::watch::Receiver<bool>,
    registration: ActiveDiscoveryRequestRegistration,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = shell.clone();
    run_shell_discovery_off_runtime(move || {
        let _registration = registration;
        shell.continue_provider_discovery_with_credential_dispatch(
            input,
            credential,
            dispatch_lease,
            cancelled,
        )
    })
    .await
}

async fn supply_provider_discovery_curl_evidence_off_runtime(
    shell: &shell::ShellApi,
    session_id: String,
    expected_revision: u64,
    curl: shell::SecretProviderCurl,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = shell.clone();
    run_shell_discovery_off_runtime(move || {
        shell.supply_provider_discovery_curl_evidence(&session_id, expected_revision, curl)
    })
    .await
}

fn discovery_credential_lease_binding(
    shell: &shell::ShellApi,
    session: &shell::ProviderDiscoverySessionDto,
    expected_revision: u64,
) -> CommandResult<DiscoveryCredentialLeaseBinding> {
    if session.revision != expected_revision || !session.credential_binding_requested {
        return Err(CommandError::invalid_input());
    }
    let context = shell.get_provider_discovery_credential_lease_context(&session.id)?;
    if context.session_id != session.id || context.connection_id != session.connection_id {
        return Err(CommandError::internal());
    }
    Ok(DiscoveryCredentialLeaseBinding {
        session_id: context.session_id,
        connection_id: context.connection_id,
        credential_origin_approval_id: context.credential_origin_approval_id,
        credential_origin_grant_sha256: context.credential_origin_grant_sha256,
        connection_binding_sha256: context.connection_binding_sha256,
    })
}

pub(crate) async fn discovery_credential_status(
    app: &AppHandle,
    state: &AppState,
    shell: &shell::ShellApi,
    session_id: &str,
    expected_revision: u64,
) -> CommandResult<CredentialStatus> {
    let session = shell.get_provider_discovery(session_id)?;
    if session.revision != expected_revision || !session.credential_binding_requested {
        return Err(CommandError::invalid_input());
    }
    if session.state != "committing" {
        let binding = discovery_credential_lease_binding(shell, &session, expected_revision)?;
        return Ok(state.discovery_credential_lease_status(&binding));
    }

    let context = shell.get_provider_discovery_credential_install_context(session_id)?;
    if context.session_id != session.id
        || context.session_revision != session.revision
        || context.connection_id != session.connection_id
        || session.commit_attempt_id.as_deref() != Some(context.commit_attempt_id.as_str())
        || session.commit_plan_sha256.as_deref() != Some(context.commit_plan_sha256.as_str())
    {
        return Err(CommandError::internal());
    }
    discovery_committing_credential_status_with(&PlatformDiscoveryCredentialVault { app }, &context)
        .await
}

async fn discovery_committing_credential_status_with(
    vault: &dyn DiscoveryCredentialVault,
    context: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<CredentialStatus> {
    match (
        context.operation_status.as_str(),
        context.native_execution_id.as_ref(),
    ) {
        // Prepared has no physical slot to inspect. In particular, a slot
        // from a rolled-back execution must not be projected as available.
        ("prepared", None) => Ok(CredentialStatus::Missing),
        ("started", Some(_)) => Ok(status_only_bound_observation(
            vault
                .observe_bound(
                    &context.connection_id,
                    discovery_credential_authority(context)?,
                )
                .await,
        )),
        _ => Ok(CredentialStatus::Unreadable),
    }
}

pub(crate) async fn capture_discovery_credential_for_empty_bound_slot(
    app: &AppHandle,
    reference: &str,
    authority: &CredentialAuthority,
) -> CommandResult<(NativeCaptureStatus, NativeCredential)> {
    let captured = capture_discovery_credential_for_empty_bound_slot_with(
        &PlatformDiscoveryCredentialVault { app },
        reference,
        authority,
    )
    .await?;
    Ok((captured.status, captured.value))
}

async fn capture_discovery_credential_for_empty_bound_slot_with(
    vault: &dyn DiscoveryCredentialVault,
    reference: &str,
    authority: &CredentialAuthority,
) -> CommandResult<CapturedDiscoveryCredential> {
    require_missing_discovery_bound_slot(vault, reference, authority).await?;
    let captured = vault.capture_bound().await?;
    require_missing_discovery_bound_slot(vault, reference, authority).await?;
    Ok(captured)
}

async fn require_missing_discovery_bound_slot(
    vault: &dyn DiscoveryCredentialVault,
    reference: &str,
    authority: &CredentialAuthority,
) -> CommandResult<()> {
    if vault.status_bound(reference, authority.clone()).await? == CredentialStatus::Missing {
        Ok(())
    } else {
        Err(CommandError::invalid_input())
    }
}

pub(crate) async fn capture_precommit_discovery_credential(
    app: &AppHandle,
    state: &AppState,
    shell: &shell::ShellApi,
    session_id: &str,
    expected_revision: u64,
) -> CommandResult<NativeCaptureStatus> {
    capture_precommit_discovery_credential_with(
        &PlatformDiscoveryCredentialVault { app },
        state,
        shell,
        session_id,
        expected_revision,
    )
    .await
}

async fn capture_precommit_discovery_credential_with(
    vault: &dyn DiscoveryCredentialVault,
    state: &AppState,
    shell: &shell::ShellApi,
    session_id: &str,
    expected_revision: u64,
) -> CommandResult<NativeCaptureStatus> {
    let session = shell.get_provider_discovery(session_id)?;
    if session.state == "committing" {
        return Err(CommandError::invalid_input());
    }
    let binding = discovery_credential_lease_binding(shell, &session, expected_revision)?;
    let captured = vault.capture_bound().await?;
    state.install_discovery_credential_lease(binding, captured.value)?;
    Ok(captured.status)
}

/// Moves one exact process-local discovery credential into the existing
/// durable install WAL. The WAL is marked started before the runtime entry is
/// invalidated or the native vault is written.
async fn promote_discovery_credential_lease(
    app: &AppHandle,
    state: &AppState,
    shell: &shell::ShellApi,
    session: &shell::ProviderDiscoverySessionDto,
) -> CommandResult<bool> {
    if session.state != "committing" || !session.credential_binding_requested {
        return Ok(false);
    }
    let candidate = DiscoveryCredentialCommitCandidate {
        session_id: session.id.clone(),
        session_revision: session.revision,
        connection_id: session.connection_id.clone(),
        commit_attempt_id: session
            .commit_attempt_id
            .clone()
            .ok_or_else(CommandError::internal)?,
        commit_plan_sha256: session
            .commit_plan_sha256
            .clone()
            .ok_or_else(CommandError::internal)?,
    };
    promote_discovery_credential_lease_with(
        &PlatformDiscoveryCredentialVault { app },
        state,
        shell,
        &candidate,
    )
    .await
}

async fn promote_discovery_credential_lease_with(
    vault: &dyn DiscoveryCredentialVault,
    state: &AppState,
    journal: &dyn DiscoveryCredentialInstallJournal,
    candidate: &DiscoveryCredentialCommitCandidate,
) -> CommandResult<bool> {
    let context = journal.install_context(&candidate.session_id)?;
    validate_discovery_commit_context(candidate, &context)?;
    if context.operation_status == "started" {
        require_started_discovery_credential_install(&context)?;
        if state.discovery_credential_lease_matches_commit(
            &candidate.session_id,
            &context.connection_id,
            &context.connection_binding_sha256,
        )? {
            // Started owns the native side effect already. A surviving runtime
            // lease would be ambiguous and must never trigger a second store.
            state.clear_discovery_credential_lease(&candidate.session_id);
            return Err(CommandError::internal());
        }
        return Ok(false);
    }
    if context.operation_status != "prepared"
        || context.native_execution_reservation_id.is_some()
        || context.native_execution_id.is_some()
    {
        return Err(CommandError::internal());
    }
    if !state.discovery_credential_lease_matches_commit(
        &candidate.session_id,
        &context.connection_id,
        &context.connection_binding_sha256,
    )? {
        return Ok(false);
    }
    let reserved = journal.reserve_install(
        &candidate.session_id,
        candidate.session_revision,
        &context.operation_id,
        &context.commit_attempt_id,
        &context.commit_plan_sha256,
    )?;
    validate_reserved_discovery_install_context(&context, &reserved)?;
    let authority = discovery_credential_reservation_authority(&reserved)?;
    require_missing_discovery_bound_slot(vault, &reserved.connection_id, &authority).await?;
    // Recheck after the asynchronous reservation boundary and immediately
    // before consuming the process-local secret. A restored or competing
    // exact slot must remain visible to recovery rather than being adopted or
    // handed to the native store under this new execution authority.
    require_missing_discovery_bound_slot(vault, &reserved.connection_id, &authority).await?;
    let credential = state
        .take_discovery_credential_lease_for_commit(
            &candidate.session_id,
            &reserved.connection_id,
            &reserved.connection_binding_sha256,
        )?
        .ok_or_else(CommandError::internal)?;
    let reservation_id = reserved
        .native_execution_reservation_id
        .as_deref()
        .ok_or_else(CommandError::internal)?;
    let prepared_store =
        vault.prepare_bound_store(&reserved.connection_id, credential, &authority)?;
    // Everything that can fail without attempting the native write is above
    // this cutpoint. Started is the durable store-attempt marker, so invoke
    // exactly one store immediately after Core accepts this exact reservation.
    let started = journal.start_install(
        &candidate.session_id,
        candidate.session_revision,
        &reserved.operation_id,
        &reserved.commit_attempt_id,
        &reserved.commit_plan_sha256,
        reservation_id,
    )?;
    if !discovery_install_start_is_exact(&started, &reserved, reservation_id, &authority) {
        return Err(CommandError::internal());
    }
    let store_result = vault.store_prepared(prepared_store).await;
    if platform_result_requires_credential_recovery(&store_result) {
        mark_discovery_credential_store_durability_unknown(
            journal, candidate, &started, &authority,
        )?;
        return Err(store_result
            .expect_err("recovery-required result is an error")
            .into());
    }
    let observation = vault
        .observe_bound(&reserved.connection_id, authority.clone())
        .await;
    settle_discovery_credential_store_attempt(
        journal,
        candidate,
        &started,
        &authority,
        store_result,
        observation,
    )
}

fn settle_discovery_credential_store_attempt(
    journal: &dyn DiscoveryCredentialInstallJournal,
    candidate: &DiscoveryCredentialCommitCandidate,
    started: &shell::ProviderDiscoveryCredentialInstallContextDto,
    authority: &CredentialAuthority,
    store_result: PlatformResult<()>,
    observation: PlatformResult<BoundCredentialObservation>,
) -> CommandResult<bool> {
    match observation {
        Ok(BoundCredentialObservation::Match) => Ok(true),
        Ok(BoundCredentialObservation::Missing) => {
            journal.attest_no_effect(
                &candidate.session_id,
                &started.operation_id,
                &started.commit_attempt_id,
                &started.commit_plan_sha256,
                authority.authority_id(),
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

fn mark_discovery_credential_store_durability_unknown(
    journal: &dyn DiscoveryCredentialInstallJournal,
    candidate: &DiscoveryCredentialCommitCandidate,
    started: &shell::ProviderDiscoveryCredentialInstallContextDto,
    authority: &CredentialAuthority,
) -> CommandResult<()> {
    journal.mark_durability_unknown(
        &candidate.session_id,
        started.session_revision,
        &started.operation_id,
        &started.commit_attempt_id,
        &started.commit_plan_sha256,
        authority.authority_id(),
        &started.connection_id,
        &started.connection_binding_sha256,
    )
}

fn validate_discovery_commit_context(
    candidate: &DiscoveryCredentialCommitCandidate,
    context: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<()> {
    if context.session_id != candidate.session_id
        || context.session_revision != candidate.session_revision
        || context.connection_id != candidate.connection_id
        || context.commit_attempt_id != candidate.commit_attempt_id
        || context.commit_plan_sha256 != candidate.commit_plan_sha256
        || context.commit_phase != "prepared"
    {
        return Err(CommandError::internal());
    }
    Ok(())
}

fn validate_reserved_discovery_install_context(
    initial: &shell::ProviderDiscoveryCredentialInstallContextDto,
    reserved: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<()> {
    if reserved.operation_status != "prepared"
        || reserved.native_execution_id.is_some()
        || reserved.commit_phase != "prepared"
        || reserved.operation_id != initial.operation_id
        || reserved.commit_attempt_id != initial.commit_attempt_id
        || reserved.commit_plan_sha256 != initial.commit_plan_sha256
        || reserved.connection_id != initial.connection_id
        || reserved.connection_binding_sha256 != initial.connection_binding_sha256
    {
        return Err(CommandError::internal());
    }
    Ok(())
}

fn discovery_install_start_is_exact(
    started: &shell::ProviderDiscoveryCredentialInstallContextDto,
    reserved: &shell::ProviderDiscoveryCredentialInstallContextDto,
    reservation_id: &str,
    authority: &CredentialAuthority,
) -> bool {
    started.operation_status == "started"
        && started.commit_phase == "prepared"
        && started.operation_id == reserved.operation_id
        && started.commit_attempt_id == reserved.commit_attempt_id
        && started.commit_plan_sha256 == reserved.commit_plan_sha256
        && started.connection_id == reserved.connection_id
        && started.connection_binding_sha256 == reserved.connection_binding_sha256
        && started.native_execution_reservation_id.as_deref() == Some(reservation_id)
        && started.native_execution_id.as_deref() == Some(authority.authority_id())
}

fn discovery_action_requires_runtime_credential(
    session: &shell::ProviderDiscoverySessionDto,
    action: &shell::ContinueProviderDiscoveryActionInput,
) -> bool {
    session.credential_binding_requested
        && matches!(
            (session.state.as_str(), action),
            (
                "awaiting_credential_origin_approval",
                shell::ContinueProviderDiscoveryActionInput::ApproveCredentialOrigin { .. }
            ) | (
                "awaiting_probe_consent",
                shell::ContinueProviderDiscoveryActionInput::ApproveProbes { .. }
            ) | (
                "interrupted",
                shell::ContinueProviderDiscoveryActionInput::RestartInterrupted
            )
        )
        && (session.state != "interrupted"
            || matches!(
                session.recovery_operation.as_deref(),
                Some("list_models" | "probe_capabilities")
            ))
}

fn credential_for_discovery_action(
    state: &AppState,
    shell: &shell::ShellApi,
    session: &shell::ProviderDiscoverySessionDto,
    expected_revision: u64,
    action: &shell::ContinueProviderDiscoveryActionInput,
) -> CommandResult<Option<shell::SecretCredential>> {
    if !discovery_action_requires_runtime_credential(session, action) {
        return Ok(None);
    }
    let binding = discovery_credential_lease_binding(shell, session, expected_revision)?;
    state
        .discovery_credential_for_request(&binding)?
        .ok_or_else(CommandError::invalid_input)
        .map(Some)
}

pub(crate) fn discovery_credential_authority(
    context: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<CredentialAuthority> {
    if context.operation_status != "started"
        || context.native_execution_reservation_id.as_deref()
            != context.native_execution_id.as_deref()
    {
        return Err(CommandError::invalid_input());
    }
    let native_execution_id = context
        .native_execution_id
        .clone()
        .ok_or_else(CommandError::invalid_input)?;
    CredentialAuthority::new(
        native_execution_id,
        context.connection_binding_sha256.clone(),
    )
    .map_err(Into::into)
}

pub(crate) fn discovery_credential_reservation_authority(
    context: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<CredentialAuthority> {
    if context.operation_status != "prepared" || context.native_execution_id.is_some() {
        return Err(CommandError::invalid_input());
    }
    let reservation_id = context
        .native_execution_reservation_id
        .clone()
        .ok_or_else(CommandError::invalid_input)?;
    CredentialAuthority::new(reservation_id, context.connection_binding_sha256.clone())
        .map_err(Into::into)
}

fn discovery_compensation_credential_authority(
    context: &shell::ProviderDiscoveryCredentialAuthorityDto,
) -> CommandResult<CredentialAuthority> {
    CredentialAuthority::new(
        context.native_execution_id.clone(),
        context.connection_binding_sha256.clone(),
    )
    .map_err(Into::into)
}

fn discovery_compensation_confirmation_context(
    session: &shell::ProviderDiscoverySessionDto,
    authority: &shell::ProviderDiscoveryCredentialAuthorityDto,
) -> CommandResult<NativeCredentialEffectContext> {
    if authority.connection_id != session.connection_id {
        return Err(CommandError::invalid_input());
    }
    let mut state_hasher = Sha256::new();
    let session_revision = session.revision.to_string();
    state_hasher.update(b"dev.lorepia.discovery-compensation-confirmation.v1\0");
    for value in [
        session.id.as_bytes(),
        session_revision.as_bytes(),
        authority.operation_id.as_bytes(),
        authority.native_execution_id.as_bytes(),
        authority.commit_attempt_id.as_bytes(),
        authority.connection_id.as_bytes(),
        authority.credential_api_origin.as_bytes(),
        authority.credential_origin_approval_id.as_bytes(),
        authority.credential_origin_grant_sha256.as_bytes(),
        authority.connection_binding_sha256.as_bytes(),
    ] {
        state_hasher.update(value);
        state_hasher.update([0]);
    }
    let state_sha256 = format!("{:x}", state_hasher.finalize());
    NativeCredentialEffectContext::new(
        NativeCredentialEffect::DiscoveryCompensation,
        session.connection_id.clone(),
        authority.credential_api_origin.clone(),
        format!(
            "compensation_state_sha256={state_sha256};session_revision={}",
            session.revision
        ),
    )
    .map_err(Into::into)
}

const fn bound_observation_status(observation: BoundCredentialObservation) -> CredentialStatus {
    match observation {
        BoundCredentialObservation::Missing => CredentialStatus::Missing,
        BoundCredentialObservation::Match => CredentialStatus::Available,
        BoundCredentialObservation::Legacy
        | BoundCredentialObservation::Mismatch
        | BoundCredentialObservation::Unreadable => CredentialStatus::Unreadable,
    }
}

/// Status and recovery classification must remain fail-closed without turning
/// an unreadable platform item into a bootstrap outage. Mutation and provider
/// dispatch paths continue to propagate the original platform error.
pub(crate) fn status_only_bound_observation(
    observation: PlatformResult<BoundCredentialObservation>,
) -> CredentialStatus {
    observation.map_or(CredentialStatus::Unreadable, bound_observation_status)
}

fn native_credential_to_shell(value: Option<NativeCredential>) -> Option<shell::SecretCredential> {
    value.map(|value| shell::SecretCredential::new(value.into_secret_string()))
}

async fn drive_provider_discovery_compensation_observe_only(
    app: &AppHandle,
    shell: &shell::ShellApi,
    session: shell::ProviderDiscoverySessionDto,
    allow_failed_retry: bool,
    observe_error_policy: CompensationObserveErrorPolicy,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    match drive_provider_discovery_compensation_with(
        &PlatformDiscoveryCredentialVault { app },
        shell,
        session,
        allow_failed_retry,
        CompensationCredentialEffectPolicy::ObserveOnly,
        observe_error_policy,
        None,
    )
    .await?
    {
        DiscoveryCompensationDriveResult::Finished(session) => Ok(session),
        DiscoveryCompensationDriveResult::NativeConfirmationRequired { .. } => {
            Err(CommandError::internal())
        }
    }
}

/// Runs the observation pass under the writer, presents the trusted modal
/// with no credential lock held, then reacquires and repeats every durable and
/// native precondition before consuming the one-use receipt and deleting.
async fn drive_provider_discovery_compensation_explicit(
    app: &AppHandle,
    state: &AppState,
    shell: &shell::ShellApi,
    expected_session: shell::ProviderDiscoverySessionDto,
    allow_failed_retry: bool,
    observe_error_policy: CompensationObserveErrorPolicy,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let vault = PlatformDiscoveryCredentialVault { app };
    let initial = {
        let _operation = state.lock_provider_credential_operation().await;
        let latest = shell.get_provider_discovery(&expected_session.id)?;
        if latest != expected_session {
            return Err(CommandError::invalid_input());
        }
        drive_provider_discovery_compensation_with(
            &vault,
            shell,
            latest,
            allow_failed_retry,
            CompensationCredentialEffectPolicy::RequireNativeConfirmation,
            observe_error_policy,
            None,
        )
        .await?
    };
    let (prompted_session, prompt_context) = match initial {
        DiscoveryCompensationDriveResult::Finished(session) => return Ok(session),
        DiscoveryCompensationDriveResult::NativeConfirmationRequired { session, context } => {
            (session, context)
        }
    };

    // Deliberately outside the global writer. Cancel, focus loss, and native
    // presentation failure all return here without starting the durable step.
    let confirmation = vault.confirm_compensation(prompt_context).await?;

    let _operation = state.lock_provider_credential_operation().await;
    let latest = shell.get_provider_discovery(&prompted_session.id)?;
    if latest != prompted_session {
        return Err(CommandError::invalid_input());
    }
    match drive_provider_discovery_compensation_with(
        &vault,
        shell,
        latest,
        allow_failed_retry,
        CompensationCredentialEffectPolicy::RequireNativeConfirmation,
        observe_error_policy,
        Some(confirmation),
    )
    .await?
    {
        DiscoveryCompensationDriveResult::Finished(session) => Ok(session),
        DiscoveryCompensationDriveResult::NativeConfirmationRequired { .. } => {
            Err(CommandError::invalid_input())
        }
    }
}

#[allow(clippy::too_many_lines)] // Keeps the observe-confirm-reobserve-delete state machine linear.
async fn drive_provider_discovery_compensation_with(
    vault: &dyn DiscoveryCredentialVault,
    shell: &shell::ShellApi,
    session: shell::ProviderDiscoverySessionDto,
    allow_failed_retry: bool,
    effect_policy: CompensationCredentialEffectPolicy,
    observe_error_policy: CompensationObserveErrorPolicy,
    confirmation: Option<DiscoveryCompensationConfirmation>,
) -> CommandResult<DiscoveryCompensationDriveResult> {
    if session.state != "compensating" {
        return Ok(DiscoveryCompensationDriveResult::Finished(session));
    }
    let attempt_id = session
        .commit_attempt_id
        .as_deref()
        .ok_or_else(CommandError::internal)?;
    let steps = shell.list_provider_discovery_compensation_steps(attempt_id)?;
    let mut credential_steps = steps
        .iter()
        .filter(|step| step.kind == "remove_credential_slot");
    let credential_step = credential_steps.next();
    if credential_steps.next().is_some()
        || credential_step.is_some_and(|step| step.commit_attempt_id != attempt_id)
    {
        return Err(CommandError::internal());
    }
    // The DTO deliberately withholds the native slot target. Core revalidates
    // this exact step ID against the session's immutable commit plan before it
    // lets the backend claim the step; only then may the backend use the
    // session-bound connection ID as the opaque native credential reference.
    let Some(step) = credential_step else {
        if session.credential_binding_requested {
            return Err(CommandError::internal());
        }
        return shell
            .continue_provider_discovery_compensation(&session.id)
            .map(DiscoveryCompensationDriveResult::Finished)
            .map_err(Into::into);
    };

    match step.status.as_str() {
        "completed" => {
            return shell
                .continue_provider_discovery_compensation(&session.id)
                .map(DiscoveryCompensationDriveResult::Finished)
                .map_err(Into::into);
        }
        "pending" if step.attempt_count == 0 => {}
        "failed" if allow_failed_retry => {}
        "pending" | "in_progress" | "failed" | "outcome_unknown" => {
            return Ok(DiscoveryCompensationDriveResult::Finished(session));
        }
        _ => return Err(CommandError::internal()),
    }

    let authority_context =
        shell.get_provider_discovery_credential_compensation_authority(&session.id)?;
    if authority_context.operation_id.is_empty()
        || authority_context.commit_attempt_id != attempt_id
        || authority_context.connection_id != session.connection_id
    {
        return Err(CommandError::internal());
    }
    let authority = discovery_compensation_credential_authority(&authority_context)?;
    let Some(preflight) = observe_discovery_compensation_slot(
        vault,
        &session.connection_id,
        &authority,
        observe_error_policy,
    )
    .await?
    else {
        // A status/read backend outage is not evidence that this exact slot
        // is absent. Leave the pending step untouched so startup can publish
        // and a later recovery pass can retry it.
        return Ok(DiscoveryCompensationDriveResult::Finished(session));
    };
    if preflight == BoundCredentialObservation::Match {
        if effect_policy == CompensationCredentialEffectPolicy::ObserveOnly {
            // Startup may observe and publish non-effect progress, but a
            // database-derived target never gains unattended delete authority.
            return Ok(DiscoveryCompensationDriveResult::Finished(session));
        }
        let Some(confirmation) = confirmation else {
            let context =
                discovery_compensation_confirmation_context(&session, &authority_context)?;
            return Ok(
                DiscoveryCompensationDriveResult::NativeConfirmationRequired { session, context },
            );
        };
        let context = discovery_compensation_confirmation_context(&session, &authority_context)?;
        confirmation.consume_exact(&context)?;
    }
    let started = shell.start_provider_discovery_credential_compensation(&session.id, &step.id)?;
    if started.id != step.id
        || started.commit_attempt_id != attempt_id
        || started.kind != "remove_credential_slot"
        || started.status != "in_progress"
    {
        return Err(CommandError::internal());
    }

    let result = match preflight {
        BoundCredentialObservation::Missing => {
            complete_provider_discovery_credential_compensation(shell, &session, &step.id)
        }
        BoundCredentialObservation::Legacy
        | BoundCredentialObservation::Mismatch
        | BoundCredentialObservation::Unreadable => shell
            .mark_provider_discovery_credential_compensation_unknown(&session.id, &step.id)
            .map_err(Into::into),
        BoundCredentialObservation::Match => {
            let (delete_result, postflight) =
                delete_and_observe_discovery_bound_slot(vault, &session.connection_id, &authority)
                    .await;
            match credential_compensation_delete_outcome(&delete_result, &postflight) {
                CredentialCompensationDeleteOutcome::Complete => {
                    complete_provider_discovery_credential_compensation(shell, &session, &step.id)
                }
                CredentialCompensationDeleteOutcome::Fail => shell
                    .fail_provider_discovery_credential_compensation(
                        &session.id,
                        &step.id,
                        credential_compensation_failure(
                            "credential_compensation_delete_failed",
                            "provider.discovery.credential_compensation_delete_failed",
                        ),
                    )
                    .map_err(Into::into),
                CredentialCompensationDeleteOutcome::Unknown => shell
                    .mark_provider_discovery_credential_compensation_unknown(&session.id, &step.id)
                    .map_err(Into::into),
            }
        }
    }?;
    Ok(DiscoveryCompensationDriveResult::Finished(result))
}

fn credential_compensation_delete_outcome(
    delete_result: &PlatformResult<()>,
    postflight: &PlatformResult<BoundCredentialObservation>,
) -> CredentialCompensationDeleteOutcome {
    if platform_result_requires_credential_recovery(delete_result) {
        return CredentialCompensationDeleteOutcome::Unknown;
    }
    match (delete_result, postflight) {
        (_, Ok(BoundCredentialObservation::Missing)) => {
            CredentialCompensationDeleteOutcome::Complete
        }
        (Err(_), Ok(BoundCredentialObservation::Match)) => {
            CredentialCompensationDeleteOutcome::Fail
        }
        _ => CredentialCompensationDeleteOutcome::Unknown,
    }
}

fn platform_result_requires_credential_recovery(result: &PlatformResult<()>) -> bool {
    matches!(
        result,
        Err(error) if error.code() == PlatformErrorCode::CredentialRecoveryRequired
    )
}

async fn observe_discovery_compensation_slot(
    vault: &dyn DiscoveryCredentialVault,
    reference: &str,
    authority: &CredentialAuthority,
    error_policy: CompensationObserveErrorPolicy,
) -> CommandResult<Option<BoundCredentialObservation>> {
    match vault.observe_bound(reference, authority.clone()).await {
        Ok(observation) => Ok(Some(observation)),
        Err(_) if error_policy == CompensationObserveErrorPolicy::Defer => Ok(None),
        Err(error) => Err(error.into()),
    }
}

async fn delete_and_observe_discovery_bound_slot(
    vault: &dyn DiscoveryCredentialVault,
    reference: &str,
    authority: &CredentialAuthority,
) -> (
    PlatformResult<()>,
    PlatformResult<BoundCredentialObservation>,
) {
    let delete_result = vault.delete_bound(reference, authority.clone()).await;
    let postflight = vault.observe_bound(reference, authority.clone()).await;
    (delete_result, postflight)
}

fn complete_provider_discovery_credential_compensation(
    shell: &shell::ShellApi,
    session: &shell::ProviderDiscoverySessionDto,
    step_id: &str,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    match shell.complete_provider_discovery_credential_compensation(&session.id, step_id) {
        Ok(session) => Ok(session),
        Err(_) => shell
            .fail_provider_discovery_credential_compensation(
                &session.id,
                step_id,
                credential_compensation_failure(
                    "credential_compensation_record_failed",
                    "provider.discovery.credential_compensation_record_failed",
                ),
            )
            .map_err(Into::into),
    }
}

fn credential_compensation_failure(code: &str, message_key: &str) -> shell::DiscoveryFailureDto {
    shell::DiscoveryFailureDto {
        code: code.to_owned(),
        message_key: message_key.to_owned(),
        recoverable: true,
    }
}

fn find_connection(
    shell: &shell::ShellApi,
    connection_id: &str,
) -> CommandResult<shell::ProviderConnectionDto> {
    shell
        .list_provider_connections()?
        .into_iter()
        .find(|connection| connection.id == connection_id)
        .ok_or_else(CommandError::invalid_input)
}

fn bounded_secret_curl(value: String) -> CommandResult<shell::SecretProviderCurl> {
    if value.len() > MAXIMUM_PROVIDER_CURL_BYTES || value.trim().is_empty() {
        return Err(CommandError::invalid_input());
    }
    Ok(shell::SecretProviderCurl::new(value))
}

#[cfg(test)]
mod tests {
    include!("provider_commands/tests/support.rs");
    include!("provider_commands/tests/provider_access.rs");
    include!("provider_commands/tests/credential_install.rs");
    include!("provider_commands/tests/compensation.rs");
}
