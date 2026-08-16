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
    use std::{
        collections::{BTreeMap, BTreeSet},
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        path::Path,
        sync::{Arc, Mutex, mpsc},
        thread,
        time::Duration,
    };

    use lorepia_shell_api as shell;
    use serde_json::json;
    use tauri_plugin_lorepia_platform::{
        BoundCredentialObservation, ClipboardCleanupStatus, CredentialAuthority, CredentialStatus,
        NativeCaptureStatus, NativeCredential, NativeCredentialEffect,
        NativeCredentialEffectContext, PlatformError, PlatformErrorCode, PlatformResult,
    };
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::{
        CancelProviderDiscoveryRequest, CapturedDiscoveryCredential,
        CompensationCredentialEffectPolicy, CompensationObserveErrorPolicy,
        ConnectionSlotGuardFuture, CredentialCompensationDeleteOutcome,
        CredentialInstallRecoveryAction, DiscoveryCompensationConfirmation,
        DiscoveryCompensationDriveResult, DiscoveryCredentialCommitCandidate,
        DiscoveryCredentialInstallJournal, DiscoveryCredentialVault, DiscoveryVaultFuture,
        ExistingConnectionCredentialReadFuture, ExistingConnectionCredentialReader,
        MAXIMUM_PROVIDER_CURL_BYTES, NewConnectionSlotGuard,
        PollProviderDiscoveryEventsForSessionRequest, PreparedDiscoveryCredentialStore,
        ProviderDiscoverySessionRequest, begin_provider_discovery_curl_with_reader,
        begin_provider_discovery_with_reader, bounded_secret_curl,
        capture_discovery_credential_for_empty_bound_slot_with,
        capture_precommit_discovery_credential_with, continue_provider_discovery_off_runtime,
        create_provider_connection_with_slot_guard, credential_compensation_delete_outcome,
        credential_for_discovery_action, credential_install_recovery_action,
        delete_and_observe_discovery_bound_slot, discovery_committing_credential_status_with,
        discovery_compensation_confirmation_context, discovery_compensation_credential_authority,
        discovery_credential_authority, drive_provider_discovery_compensation_with,
        observe_discovery_compensation_slot, promote_discovery_credential_lease_with,
        recover_provider_discovery_credential_installs, register_active_discovery_request,
        request_provider_discovery_cancellation, require_started_discovery_credential_install,
        run_provider_discovery_assistant_turn, run_shell_discovery_off_runtime,
        settle_started_discovery_credential_recovery, start_provider_model_sync_with_reader,
        status_only_bound_observation, supply_provider_discovery_curl_evidence_off_runtime,
    };
    use crate::{
        error::{CommandError, CommandResult},
        state::{AppState, DiscoveryCredentialLeaseBinding},
    };

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct FakeDiscoveryBoundKey {
        reference: String,
        authority_id: String,
        binding_sha256: String,
    }

    impl FakeDiscoveryBoundKey {
        fn new(reference: &str, authority: &CredentialAuthority) -> Self {
            Self {
                reference: reference.to_owned(),
                authority_id: authority.authority_id().to_owned(),
                binding_sha256: authority.binding_sha256().to_owned(),
            }
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum FakeDiscoveryVaultFault {
        Status,
        Observe,
        PrepareStore,
        StoreAfterEffect,
        StoreRecoveryRequiredAfterEffect,
        DeleteAfterEffect,
    }

    struct FakeDiscoveryVaultState {
        raw_slots: BTreeMap<String, CredentialStatus>,
        bound_slots: BTreeMap<FakeDiscoveryBoundKey, BoundCredentialObservation>,
        bound_slot_to_insert_on_capture: Option<FakeDiscoveryBoundKey>,
        bound_slot_to_insert_after_status: Option<FakeDiscoveryBoundKey>,
        rolled_back_bound_slot_to_restore_before_store: Option<FakeDiscoveryBoundKey>,
        captured_secret: String,
        faults: BTreeSet<FakeDiscoveryVaultFault>,
    }

    struct FakeDiscoveryVault {
        state: Mutex<FakeDiscoveryVaultState>,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeExistingConnectionCredentialReader {
        read: Mutex<Option<crate::credential_operations::ProviderConnectionCredentialRead>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl FakeExistingConnectionCredentialReader {
        fn new(
            read: Option<crate::credential_operations::ProviderConnectionCredentialRead>,
        ) -> Self {
            Self {
                read: Mutex::new(read),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl ExistingConnectionCredentialReader for FakeExistingConnectionCredentialReader {
        fn read<'a>(
            &'a self,
            _shell: &'a shell::ShellApi,
            connection_id: &'a str,
        ) -> ExistingConnectionCredentialReadFuture<'a> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .expect("fake existing credential calls")
                    .push(connection_id.to_owned());
                self.read
                    .lock()
                    .expect("fake existing credential read")
                    .take()
                    .ok_or_else(CommandError::invalid_input)
            })
        }
    }

    impl FakeDiscoveryVault {
        fn new(calls: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                state: Mutex::new(FakeDiscoveryVaultState {
                    raw_slots: BTreeMap::new(),
                    bound_slots: BTreeMap::new(),
                    bound_slot_to_insert_on_capture: None,
                    bound_slot_to_insert_after_status: None,
                    rolled_back_bound_slot_to_restore_before_store: None,
                    captured_secret: "synthetic-discovery-secret".to_owned(),
                    faults: BTreeSet::new(),
                }),
                calls,
            }
        }

        fn insert_raw(&self, reference: &str) {
            self.state
                .lock()
                .expect("fake vault")
                .raw_slots
                .insert(reference.to_owned(), CredentialStatus::Available);
        }

        fn raw_status(&self, reference: &str) -> CredentialStatus {
            self.state
                .lock()
                .expect("fake vault")
                .raw_slots
                .get(reference)
                .copied()
                .unwrap_or(CredentialStatus::Missing)
        }

        fn insert_bound(&self, reference: &str, authority: &CredentialAuthority) {
            self.state.lock().expect("fake vault").bound_slots.insert(
                FakeDiscoveryBoundKey::new(reference, authority),
                BoundCredentialObservation::Match,
            );
        }

        fn insert_bound_during_capture(&self, reference: &str, authority: &CredentialAuthority) {
            self.state
                .lock()
                .expect("fake vault")
                .bound_slot_to_insert_on_capture =
                Some(FakeDiscoveryBoundKey::new(reference, authority));
        }

        fn insert_bound_after_next_status(&self, reference: &str, authority: &CredentialAuthority) {
            self.state
                .lock()
                .expect("fake vault")
                .bound_slot_to_insert_after_status =
                Some(FakeDiscoveryBoundKey::new(reference, authority));
        }

        fn restore_rolled_back_bound_slot_before_next_store(
            &self,
            reference: &str,
            prior_execution_authority: &CredentialAuthority,
        ) {
            self.state
                .lock()
                .expect("fake vault")
                .rolled_back_bound_slot_to_restore_before_store = Some(FakeDiscoveryBoundKey::new(
                reference,
                prior_execution_authority,
            ));
        }

        fn bound_status(
            &self,
            reference: &str,
            authority: &CredentialAuthority,
        ) -> CredentialStatus {
            let state = self.state.lock().expect("fake vault");
            match state
                .bound_slots
                .get(&FakeDiscoveryBoundKey::new(reference, authority))
            {
                None => CredentialStatus::Missing,
                Some(BoundCredentialObservation::Unreadable) => CredentialStatus::Unreadable,
                Some(
                    BoundCredentialObservation::Missing
                    | BoundCredentialObservation::Legacy
                    | BoundCredentialObservation::Match
                    | BoundCredentialObservation::Mismatch,
                ) => CredentialStatus::Available,
            }
        }

        fn fail_status(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .insert(FakeDiscoveryVaultFault::Status);
        }

        fn fail_observe(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .insert(FakeDiscoveryVaultFault::Observe);
        }

        fn restore_observe(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .remove(&FakeDiscoveryVaultFault::Observe);
        }

        fn fail_store_after_effect(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .insert(FakeDiscoveryVaultFault::StoreAfterEffect);
        }

        fn require_recovery_after_store_effect(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .insert(FakeDiscoveryVaultFault::StoreRecoveryRequiredAfterEffect);
        }

        fn fail_prepare_store(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .insert(FakeDiscoveryVaultFault::PrepareStore);
        }

        fn fail_delete_after_effect(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .insert(FakeDiscoveryVaultFault::DeleteAfterEffect);
        }
    }

    impl DiscoveryCredentialVault for FakeDiscoveryVault {
        fn status_bound<'a>(
            &'a self,
            reference: &'a str,
            authority: CredentialAuthority,
        ) -> DiscoveryVaultFuture<'a, CredentialStatus> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .expect("fake calls")
                    .push("vault_bound_status");
                let mut state = self.state.lock().expect("fake vault");
                if state.faults.contains(&FakeDiscoveryVaultFault::Status) {
                    return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
                }
                let status = match state
                    .bound_slots
                    .get(&FakeDiscoveryBoundKey::new(reference, &authority))
                {
                    None => CredentialStatus::Missing,
                    Some(BoundCredentialObservation::Unreadable) => CredentialStatus::Unreadable,
                    Some(
                        BoundCredentialObservation::Missing
                        | BoundCredentialObservation::Legacy
                        | BoundCredentialObservation::Match
                        | BoundCredentialObservation::Mismatch,
                    ) => CredentialStatus::Available,
                };
                if let Some(key) = state.bound_slot_to_insert_after_status.take() {
                    state
                        .bound_slots
                        .insert(key, BoundCredentialObservation::Match);
                }
                Ok(status)
            })
        }

        fn capture_bound(&self) -> DiscoveryVaultFuture<'_, CapturedDiscoveryCredential> {
            Box::pin(async move {
                self.calls.lock().expect("fake calls").push("capture");
                let mut state = self.state.lock().expect("fake vault");
                if let Some(key) = state.bound_slot_to_insert_on_capture.take() {
                    state
                        .bound_slots
                        .insert(key, BoundCredentialObservation::Match);
                }
                Ok(CapturedDiscoveryCredential {
                    value: NativeCredential::new(state.captured_secret.clone()),
                    status: NativeCaptureStatus {
                        clipboard_cleanup: ClipboardCleanupStatus::Cleared,
                    },
                })
            })
        }

        fn prepare_bound_store(
            &self,
            reference: &str,
            value: NativeCredential,
            authority: &CredentialAuthority,
        ) -> PlatformResult<PreparedDiscoveryCredentialStore> {
            self.calls
                .lock()
                .expect("fake calls")
                .push("vault_prepare_store");
            if self
                .state
                .lock()
                .expect("fake vault")
                .faults
                .contains(&FakeDiscoveryVaultFault::PrepareStore)
            {
                return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
            }
            if value.expose() != "synthetic-discovery-secret" {
                return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
            }
            Ok(PreparedDiscoveryCredentialStore::Fake {
                reference: reference.to_owned(),
                value,
                authority: authority.clone(),
            })
        }

        fn store_prepared(
            &self,
            prepared: PreparedDiscoveryCredentialStore,
        ) -> DiscoveryVaultFuture<'_, ()> {
            Box::pin(async move {
                let (reference, value, authority) = prepared.into_fake();
                self.calls.lock().expect("fake calls").push("vault_store");
                assert_eq!(value.expose(), "synthetic-discovery-secret");
                let mut state = self.state.lock().expect("fake vault");
                if let Some(prior_execution) =
                    state.rolled_back_bound_slot_to_restore_before_store.take()
                {
                    state
                        .bound_slots
                        .insert(prior_execution, BoundCredentialObservation::Match);
                    return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
                }
                state.bound_slots.insert(
                    FakeDiscoveryBoundKey::new(&reference, &authority),
                    BoundCredentialObservation::Match,
                );
                if state
                    .faults
                    .contains(&FakeDiscoveryVaultFault::StoreAfterEffect)
                {
                    return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
                }
                if state
                    .faults
                    .contains(&FakeDiscoveryVaultFault::StoreRecoveryRequiredAfterEffect)
                {
                    return Err(PlatformError::new(
                        PlatformErrorCode::CredentialRecoveryRequired,
                    ));
                }
                Ok(())
            })
        }

        fn observe_bound<'a>(
            &'a self,
            reference: &'a str,
            authority: CredentialAuthority,
        ) -> DiscoveryVaultFuture<'a, BoundCredentialObservation> {
            Box::pin(async move {
                self.calls.lock().expect("fake calls").push("vault_observe");
                let state = self.state.lock().expect("fake vault");
                if state.faults.contains(&FakeDiscoveryVaultFault::Observe) {
                    return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
                }
                Ok(state
                    .bound_slots
                    .get(&FakeDiscoveryBoundKey::new(reference, &authority))
                    .copied()
                    .unwrap_or(BoundCredentialObservation::Missing))
            })
        }

        fn delete_bound<'a>(
            &'a self,
            reference: &'a str,
            authority: CredentialAuthority,
        ) -> DiscoveryVaultFuture<'a, ()> {
            Box::pin(async move {
                self.calls.lock().expect("fake calls").push("vault_delete");
                let mut state = self.state.lock().expect("fake vault");
                state
                    .bound_slots
                    .remove(&FakeDiscoveryBoundKey::new(reference, &authority));
                if state
                    .faults
                    .contains(&FakeDiscoveryVaultFault::DeleteAfterEffect)
                {
                    return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
                }
                Ok(())
            })
        }

        fn confirm_compensation(
            &self,
            context: NativeCredentialEffectContext,
        ) -> DiscoveryVaultFuture<'_, DiscoveryCompensationConfirmation> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .expect("fake calls")
                    .push("vault_confirm_compensation");
                Ok(DiscoveryCompensationConfirmation::Fake {
                    effect: context.effect(),
                    target_id: context.target_id().to_owned(),
                    origin: context.origin().to_owned(),
                    revision: context.revision().to_owned(),
                })
            })
        }
    }

    struct FakeDiscoveryJournal {
        context: Mutex<shell::ProviderDiscoveryCredentialInstallContextDto>,
        next_native_execution_id: Mutex<String>,
        mismatch_started_context: Mutex<bool>,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeNewConnectionSlotGuard {
        status: CredentialStatus,
        calls: Mutex<Vec<String>>,
    }

    impl FakeNewConnectionSlotGuard {
        fn new(status: CredentialStatus) -> Self {
            Self {
                status,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl NewConnectionSlotGuard for FakeNewConnectionSlotGuard {
        fn ensure_missing<'a>(&'a self, connection_id: &'a str) -> ConnectionSlotGuardFuture<'a> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .expect("fake slot calls")
                    .push(connection_id.to_owned());
                if self.status == CredentialStatus::Missing {
                    Ok(())
                } else {
                    Err(CommandError::invalid_input())
                }
            })
        }
    }

    impl FakeDiscoveryJournal {
        fn new(
            context: shell::ProviderDiscoveryCredentialInstallContextDto,
            calls: Arc<Mutex<Vec<&'static str>>>,
        ) -> Self {
            Self {
                context: Mutex::new(context),
                next_native_execution_id: Mutex::new(Uuid::new_v4().to_string()),
                mismatch_started_context: Mutex::new(false),
                calls,
            }
        }

        fn next_native_execution_id(&self) -> String {
            self.next_native_execution_id
                .lock()
                .expect("fake native execution")
                .clone()
        }

        fn mismatch_next_started_context(&self) {
            *self
                .mismatch_started_context
                .lock()
                .expect("fake started mismatch") = true;
        }
    }

    fn compensating_started_discovery_fixture(
        root: &Path,
    ) -> (
        shell::ShellApi,
        shell::ProviderDiscoverySessionDto,
        CredentialAuthority,
    ) {
        let fixture =
            shell::test_support::seed_synthetic_started_discovery_credential_install(root)
                .expect("seed exact Started discovery");
        let shell = fixture.shell;
        let started = shell
            .get_provider_discovery(&fixture.install.session_id)
            .expect("load Started session");
        let cancelled = shell
            .cancel_provider_discovery(&started.id, started.revision)
            .expect("request cancellation while commit is in flight");
        assert_eq!(cancelled.state, "committing");
        assert!(cancelled.cancellation_pending);
        shell
            .commit_provider_discovery(&cancelled.id, None)
            .expect_err("missing credential confirmation enters compensation");
        let compensating = shell
            .get_provider_discovery(&cancelled.id)
            .expect("reload compensating session");
        assert_eq!(compensating.state, "compensating");
        let authority_context = shell
            .get_provider_discovery_credential_compensation_authority(&compensating.id)
            .expect("load exact producing operation authority");
        let authority = discovery_compensation_credential_authority(&authority_context)
            .expect("validate exact compensation authority");
        (shell, compensating, authority)
    }

    #[test]
    fn discovery_compensation_confirmation_displays_backend_credential_api_origin() {
        let root = tempdir().expect("temporary root");
        let (shell, session, _authority) = compensating_started_discovery_fixture(root.path());
        let authority = shell
            .get_provider_discovery_credential_compensation_authority(&session.id)
            .expect("load compensation credential authority");
        let context = discovery_compensation_confirmation_context(&session, &authority)
            .expect("build compensation confirmation");

        assert_eq!(session.site_url, "https://docs.openrouter.example/");
        assert_eq!(
            context.origin(),
            "https://openrouter.ai",
            "credential deletion prompt must display the API origin bound to the slot"
        );
        let trusted_revision = context.revision().to_owned();
        let mut substituted_grant = authority.clone();
        substituted_grant.credential_origin_grant_sha256 = "f".repeat(64);
        assert_ne!(
            discovery_compensation_confirmation_context(&session, &substituted_grant)
                .expect("build substituted-grant compensation context")
                .revision(),
            trusted_revision,
            "compensation confirmation cannot be replayed with a substituted origin grant"
        );
        let mut substituted_binding = authority.clone();
        substituted_binding.connection_binding_sha256 = "e".repeat(64);
        assert_ne!(
            discovery_compensation_confirmation_context(&session, &substituted_binding)
                .expect("build substituted-binding compensation context")
                .revision(),
            trusted_revision,
            "compensation confirmation cannot be replayed with a substituted slot binding"
        );

        let mut same_origin_site = session.clone();
        same_origin_site.site_url = "https://openrouter.ai/".to_owned();
        let same_origin_context =
            discovery_compensation_confirmation_context(&same_origin_site, &authority)
                .expect("same-origin compensation remains valid");
        assert_eq!(same_origin_context.origin(), "https://openrouter.ai");
    }

    impl DiscoveryCredentialInstallJournal for FakeDiscoveryJournal {
        fn install_context(
            &self,
            session_id: &str,
        ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto> {
            self.calls.lock().expect("fake calls").push("wal_context");
            let context = self.context.lock().expect("fake journal").clone();
            if context.session_id != session_id {
                return Err(CommandError::invalid_input());
            }
            Ok(context)
        }

        fn reserve_install(
            &self,
            session_id: &str,
            expected_revision: u64,
            operation_id: &str,
            commit_attempt_id: &str,
            commit_plan_sha256: &str,
        ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto> {
            self.calls.lock().expect("fake calls").push("wal_reserved");
            let mut context = self.context.lock().expect("fake journal");
            if context.session_id != session_id
                || context.session_revision != expected_revision
                || context.operation_id != operation_id
                || context.commit_attempt_id != commit_attempt_id
                || context.commit_plan_sha256 != commit_plan_sha256
                || context.operation_status != "prepared"
                || context.native_execution_id.is_some()
            {
                return Err(CommandError::invalid_input());
            }
            let reservation_id = self.next_native_execution_id();
            if context.native_execution_reservation_id.is_some() {
                return Err(CommandError::invalid_input());
            }
            context.native_execution_reservation_id = Some(reservation_id);
            Ok(context.clone())
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
            self.calls.lock().expect("fake calls").push("wal_started");
            let mut context = self.context.lock().expect("fake journal");
            if context.session_id != session_id
                || context.session_revision != expected_revision
                || context.operation_id != operation_id
                || context.commit_attempt_id != commit_attempt_id
                || context.commit_plan_sha256 != commit_plan_sha256
                || context.operation_status != "prepared"
                || context.native_execution_reservation_id.as_deref()
                    != Some(native_execution_reservation_id)
                || context.native_execution_id.is_some()
            {
                return Err(CommandError::invalid_input());
            }
            context.operation_status = "started".to_owned();
            context.native_execution_id = Some(native_execution_reservation_id.to_owned());
            let mut returned = context.clone();
            if std::mem::take(
                &mut *self
                    .mismatch_started_context
                    .lock()
                    .expect("fake started mismatch"),
            ) {
                returned.connection_binding_sha256 = "f".repeat(64);
            }
            Ok(returned)
        }

        fn attest_no_effect(
            &self,
            session_id: &str,
            operation_id: &str,
            commit_attempt_id: &str,
            commit_plan_sha256: &str,
            native_execution_id: &str,
        ) -> CommandResult<()> {
            self.calls.lock().expect("fake calls").push("wal_no_effect");
            let context = self.context.lock().expect("fake journal");
            if context.session_id != session_id
                || context.operation_id != operation_id
                || context.commit_attempt_id != commit_attempt_id
                || context.commit_plan_sha256 != commit_plan_sha256
                || context.operation_status != "started"
                || context.native_execution_id.as_deref() != Some(native_execution_id)
            {
                return Err(CommandError::invalid_input());
            }
            Ok(())
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
            self.calls.lock().expect("fake calls").push("wal_unknown");
            let mut context = self.context.lock().expect("fake journal");
            if context.session_id != session_id
                || context.session_revision != expected_revision
                || context.operation_id != operation_id
                || context.commit_attempt_id != commit_attempt_id
                || context.commit_plan_sha256 != commit_plan_sha256
                || context.operation_status != "started"
                || context.native_execution_reservation_id.as_deref() != Some(native_execution_id)
                || context.native_execution_id.as_deref() != Some(native_execution_id)
                || context.connection_id != connection_id
                || context.connection_binding_sha256 != connection_binding_sha256
            {
                return Err(CommandError::invalid_input());
            }
            context.operation_status = "outcome_unknown".to_owned();
            Ok(())
        }
    }

    #[test]
    fn product_create_rejects_retained_orphan_before_shell_insert_and_missing_proceeds() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");

        for (status, connection_id) in [
            (CredentialStatus::Available, "orphan-create"),
            (CredentialStatus::Unreadable, "unreadable-create"),
        ] {
            let guard = FakeNewConnectionSlotGuard::new(status);
            let result =
                tauri::async_runtime::block_on(create_provider_connection_with_slot_guard(
                    &shell,
                    credential_connection_input(&shell, connection_id),
                    &guard,
                ));
            match result {
                Ok(_) => panic!("a retained or unreadable slot must block product create"),
                Err(error) => assert_eq!(error.code, "invalid_input"),
            }
            assert_eq!(
                *guard.calls.lock().expect("fake slot calls"),
                vec![connection_id.to_owned()]
            );
        }
        assert!(
            shell
                .list_provider_connections()
                .expect("list rejected product creates")
                .is_empty(),
            "the Shell insert must remain downstream of the slot guard"
        );

        let missing = FakeNewConnectionSlotGuard::new(CredentialStatus::Missing);
        let created = tauri::async_runtime::block_on(create_provider_connection_with_slot_guard(
            &shell,
            credential_connection_input(&shell, "missing-create"),
            &missing,
        ))
        .expect("a missing slot permits product create");
        assert_eq!(created.id, "missing-create");

        // A reset database does not authorize a retained item, even when a
        // renderer proposes a different origin for the reused slot ID.
        let reset_root = tempdir().expect("reset database root");
        let reset_shell = shell::ShellApi::open_data_root(reset_root.path()).expect("reset Shell");
        let retained = FakeNewConnectionSlotGuard::new(CredentialStatus::Available);
        let mut reset_input = credential_connection_input(&reset_shell, "retained-after-reset");
        reset_input.api_origin = "https://different-origin.example.test".to_owned();
        reset_input.approved_credential_origin = Some(reset_input.api_origin.clone());
        let reset_result = tauri::async_runtime::block_on(
            create_provider_connection_with_slot_guard(&reset_shell, reset_input, &retained),
        );
        assert!(reset_result.is_err());
        assert!(
            reset_shell
                .list_provider_connections()
                .expect("list reset product creates")
                .is_empty()
        );
    }

    #[derive(Clone, Copy)]
    enum ProductDiscoveryStart {
        Known,
        Site,
        Curl,
    }

    async fn begin_product_discovery_with_reader<R: ExistingConnectionCredentialReader + ?Sized>(
        shell: &shell::ShellApi,
        connection: &shell::ProviderConnectionDto,
        start: ProductDiscoveryStart,
        reader: &R,
    ) -> CommandResult<shell::ProviderDiscoverySessionDto> {
        match start {
            ProductDiscoveryStart::Known => {
                begin_provider_discovery_with_reader(
                    shell,
                    discovery_input(
                        connection,
                        shell::BeginProviderDiscoverySourceInput::KnownProvider {
                            template_id: connection.template_id.clone(),
                        },
                    ),
                    reader,
                )
                .await
            }
            ProductDiscoveryStart::Site => {
                begin_provider_discovery_with_reader(
                    shell,
                    discovery_input(connection, shell::BeginProviderDiscoverySourceInput::Site),
                    reader,
                )
                .await
            }
            ProductDiscoveryStart::Curl => {
                begin_provider_discovery_curl_with_reader(
                    shell,
                    discovery_curl_input(connection),
                    shell::SecretProviderCurl::new(format!(
                        "curl {}{}/models",
                        connection.api_origin.trim_end_matches('/'),
                        connection.api_base_path.as_deref().unwrap_or_default()
                    )),
                    reader,
                )
                .await
            }
        }
    }

    #[test]
    fn product_discovery_forwards_exact_authority_and_rejects_stale_reads_for_all_sources() {
        for (suffix, start) in [
            ("known", ProductDiscoveryStart::Known),
            ("site", ProductDiscoveryStart::Site),
            ("curl", ProductDiscoveryStart::Curl),
        ] {
            let root = tempdir().expect("temporary root");
            let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
            let connection_id = format!("stale-product-discovery-{suffix}");
            let connection = shell
                .create_provider_connection(credential_connection_input(&shell, &connection_id))
                .expect("create credential-bound connection");
            let cached_authority = install_provider_credential(&shell, &connection_id);
            let reader = FakeExistingConnectionCredentialReader::new(Some(
                crate::credential_operations::ProviderConnectionCredentialRead {
                    credential: Some(NativeCredential::new("cached-discovery-secret".to_owned())),
                    access_authority: cached_authority,
                },
            ));
            remove_provider_credential(&shell, &connection_id);

            let error = tauri::async_runtime::block_on(begin_product_discovery_with_reader(
                &shell,
                &connection,
                start,
                &reader,
            ))
            .expect_err("terminal removal must reject the exact cached authority");
            assert_eq!(error.code, "invalid_input");
            assert_eq!(
                *reader.calls.lock().expect("fake reader calls"),
                vec![connection_id]
            );
            assert!(
                shell
                    .list_provider_discoveries(32)
                    .expect("list rejected discoveries")
                    .is_empty()
            );
            assert!(
                shell
                    .poll_provider_discovery_events(32)
                    .expect("poll rejected discovery events")
                    .is_empty()
            );
        }
    }

    #[test]
    fn product_discovery_forwards_current_exact_authority_for_all_sources() {
        for (suffix, start) in [
            ("known", ProductDiscoveryStart::Known),
            ("site", ProductDiscoveryStart::Site),
            ("curl", ProductDiscoveryStart::Curl),
        ] {
            let root = tempdir().expect("temporary root");
            let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
            let connection_id = format!("current-product-discovery-{suffix}");
            let connection = shell
                .create_provider_connection(credential_connection_input(&shell, &connection_id))
                .expect("create credential-bound connection");
            let current_authority = install_provider_credential(&shell, &connection_id);
            let reader = FakeExistingConnectionCredentialReader::new(Some(
                crate::credential_operations::ProviderConnectionCredentialRead {
                    credential: Some(NativeCredential::new("current-discovery-secret".to_owned())),
                    access_authority: current_authority,
                },
            ));

            let session = tauri::async_runtime::block_on(begin_product_discovery_with_reader(
                &shell,
                &connection,
                start,
                &reader,
            ))
            .unwrap_or_else(|error| {
                panic!("current exact authority starts {suffix} discovery: {error:?}")
            });
            assert_eq!(session.connection_id, connection_id);
            assert_eq!(
                *reader.calls.lock().expect("fake reader calls"),
                vec![connection_id]
            );
            assert_eq!(
                shell
                    .list_provider_discoveries(32)
                    .expect("list admitted discoveries")
                    .len(),
                1
            );
            assert!(
                !shell
                    .poll_provider_discovery_events(32)
                    .expect("poll admitted discovery events")
                    .is_empty()
            );
        }
    }

    #[test]
    fn product_discovery_and_model_sync_keep_credentialless_authority_absent() {
        for (suffix, start) in [
            ("known", ProductDiscoveryStart::Known),
            ("site", ProductDiscoveryStart::Site),
            ("curl", ProductDiscoveryStart::Curl),
        ] {
            let root = tempdir().expect("temporary root");
            let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
            let connection_id = format!("credentialless-product-discovery-{suffix}");
            let connection = shell
                .create_provider_connection(credentialless_connection_input(&shell, &connection_id))
                .expect("create credentialless connection");
            let reader = FakeExistingConnectionCredentialReader::new(None);

            let session = tauri::async_runtime::block_on(begin_product_discovery_with_reader(
                &shell,
                &connection,
                start,
                &reader,
            ))
            .expect("credentialless discovery starts without reading an authority");
            assert_eq!(session.connection_id, connection_id);
            assert!(reader.calls.lock().expect("fake reader calls").is_empty());
        }

        let root = tempdir().expect("temporary model-sync root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let connection_id = "credentialless-product-model-sync";
        shell
            .create_provider_connection(credentialless_connection_input(&shell, connection_id))
            .expect("create credentialless model-sync connection");
        let reader = FakeExistingConnectionCredentialReader::new(None);
        tauri::async_runtime::block_on(start_provider_model_sync_with_reader(
            &shell,
            connection_id,
            &reader,
            None,
        ))
        .expect("credentialless model sync starts without reading an authority");
        assert!(reader.calls.lock().expect("fake reader calls").is_empty());
    }

    #[test]
    fn product_model_sync_forwards_exact_authority_and_rejects_a_stale_read() {
        let current_root = tempdir().expect("temporary current root");
        let current_shell =
            shell::ShellApi::open_data_root(current_root.path()).expect("open current Shell");
        let current_connection_id = "current-product-model-sync";
        current_shell
            .create_provider_connection(credential_connection_input(
                &current_shell,
                current_connection_id,
            ))
            .expect("create current credential-bound connection");
        let current_authority = install_provider_credential(&current_shell, current_connection_id);
        let current_reader = FakeExistingConnectionCredentialReader::new(Some(
            crate::credential_operations::ProviderConnectionCredentialRead {
                credential: Some(NativeCredential::new(
                    "current-model-sync-secret".to_owned(),
                )),
                access_authority: current_authority,
            },
        ));
        let started = tauri::async_runtime::block_on(start_provider_model_sync_with_reader(
            &current_shell,
            current_connection_id,
            &current_reader,
            None,
        ))
        .expect("current exact authority starts product model sync");
        assert!(!started.job_id.is_empty());
        assert_eq!(
            *current_reader.calls.lock().expect("fake reader calls"),
            vec![current_connection_id.to_owned()]
        );

        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let connection_id = "stale-product-model-sync";
        shell
            .create_provider_connection(credential_connection_input(&shell, connection_id))
            .expect("create credential-bound connection");
        let cached_authority = install_provider_credential(&shell, connection_id);
        let reader = FakeExistingConnectionCredentialReader::new(Some(
            crate::credential_operations::ProviderConnectionCredentialRead {
                credential: Some(NativeCredential::new("cached-model-sync-secret".to_owned())),
                access_authority: cached_authority,
            },
        ));
        remove_provider_credential(&shell, connection_id);

        let error = tauri::async_runtime::block_on(start_provider_model_sync_with_reader(
            &shell,
            connection_id,
            &reader,
            None,
        ))
        .expect_err("terminal removal must reject the exact cached model-sync authority");
        assert_eq!(error.code, "invalid_input");
        assert_eq!(
            *reader.calls.lock().expect("fake reader calls"),
            vec![connection_id.to_owned()]
        );
    }

    #[tokio::test]
    async fn model_sync_carrier_blocks_credential_mutation_until_provider_finishes() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let state = Arc::new(AppState::new(root.path().to_path_buf()));
        let connection_id = "leased-product-model-sync";
        let (origin, request_receiver, provider_release, provider_thread) =
            spawn_blocking_model_list_provider();
        shell
            .create_provider_connection(local_model_sync_connection_input(
                &shell,
                connection_id,
                &origin,
            ))
            .expect("create leased model-sync connection");
        let authority = install_provider_credential(&shell, connection_id);
        let reader = FakeExistingConnectionCredentialReader::new(Some(
            crate::credential_operations::ProviderConnectionCredentialRead {
                credential: Some(NativeCredential::new(
                    "synthetic-leased-model-sync-secret".to_owned(),
                )),
                access_authority: authority,
            },
        ));
        let dispatch_lease = state.lease_provider_credential_operation().await;
        let started = start_provider_model_sync_with_reader(
            &shell,
            connection_id,
            &reader,
            Some(shell::TaskCredentialLease::new(dispatch_lease)),
        )
        .await
        .expect("start leased model sync");
        assert!(!started.job_id.is_empty());
        let request = request_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("model-list provider entered");
        assert!(request.starts_with("GET /v1/models HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-leased-model-sync-secret\r\n")
        );

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
            tokio::time::timeout(Duration::from_millis(100), &mut mutation_acquired_receiver)
                .await
                .is_err(),
            "credential replacement/removal must wait while model sync owns in-memory A"
        );

        provider_release
            .send(())
            .expect("finish model-list provider");
        tokio::time::timeout(Duration::from_secs(2), &mut mutation_acquired_receiver)
            .await
            .expect("credential mutation released after model listing")
            .expect("credential mutation acquired write lease");
        mutation.await.expect("credential mutation task");
        provider_thread.join().expect("model-list provider thread");
    }

    #[test]
    fn product_discovery_sync_core_work_runs_outside_the_tauri_runtime() {
        tauri::async_runtime::block_on(async {
            run_shell_discovery_off_runtime(|| {
                let nested = tokio::runtime::Runtime::new()
                    .expect("create the same private runtime shape used by Core");
                nested.block_on(async {});
                Ok::<_, shell::ShellError>(())
            })
            .await
            .expect("run discovery operation off runtime");
        });
    }

    #[test]
    fn product_continue_discovery_avoids_nested_runtime_execution() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let selecting = shell
            .begin_provider_discovery(shell::BeginProviderDiscoveryInput {
                connection_id: "off-runtime-continue".to_owned(),
                display_name: "Off-runtime continue".to_owned(),
                site_url: "https://openrouter.ai/".to_owned(),
                docs_url: None,
                credential_binding_requested: true,
                preferred_assistant: None,
                connection_options: shell::ProviderDiscoveryConnectionOptionsInput {
                    values: Vec::new(),
                    api_base_path: None,
                    timeout_seconds: 30,
                    network_mode: shell::ProviderNetworkModeInput::Public,
                    local_network_approval: None,
                },
                supplied_evidence_ids: Vec::new(),
                source: shell::BeginProviderDiscoverySourceInput::KnownProvider {
                    template_id: "openrouter-v1".to_owned(),
                },
            })
            .expect("begin discovery outside Tokio");
        let candidate = shell
            .list_provider_discovery_candidates(&selecting.id)
            .expect("list discovery candidates")
            .into_iter()
            .find(|candidate| {
                matches!(
                    &candidate.summary,
                    shell::DiscoveryCandidateSummaryDto::ProviderTemplate { template_id, .. }
                        if template_id == "openrouter-v1"
                )
            })
            .expect("OpenRouter candidate");
        let next = tauri::async_runtime::block_on(continue_provider_discovery_off_runtime(
            &shell,
            shell::ContinueProviderDiscoveryInput {
                session_id: selecting.id,
                action_id: "00000000-0000-4000-8000-000000000099".to_owned(),
                expected_revision: selecting.revision,
                action: shell::ContinueProviderDiscoveryActionInput::SelectTemplate {
                    candidate_id: candidate.id,
                },
            },
            None,
        ))
        .expect("continue discovery without nesting Core runtime");
        assert_eq!(next.state, "awaiting_credential_origin_approval");
        let proposal = shell
            .get_provider_discovery_approval_proposal(&next.id)
            .expect("load credential-origin proposal")
            .expect("credential-origin proposal");
        let interrupted = tauri::async_runtime::block_on(continue_provider_discovery_off_runtime(
            &shell,
            shell::ContinueProviderDiscoveryInput {
                session_id: next.id,
                action_id: "00000000-0000-4000-8000-000000000100".to_owned(),
                expected_revision: next.revision,
                action: shell::ContinueProviderDiscoveryActionInput::ApproveCredentialOrigin {
                    approval_id: proposal.id,
                },
            },
            Some(shell::SecretCredential::new(
                "synthetic-off-runtime-credential".to_owned(),
            )),
        ))
        .expect("credential-free listing interruption must not nest Core runtime");
        assert_eq!(interrupted.state, "interrupted");
    }

    #[test]
    fn product_supply_curl_evidence_crosses_the_off_runtime_boundary() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let reader = FakeExistingConnectionCredentialReader::new(None);
        let awaiting = tauri::async_runtime::block_on(begin_provider_discovery_curl_with_reader(
            &shell,
            shell::BeginProviderDiscoveryCurlInput {
                connection_id: "off-runtime-curl-evidence".to_owned(),
                display_name: "Off-runtime cURL evidence".to_owned(),
                docs_url: None,
                credential_binding_requested: false,
                preferred_assistant: None,
                connection_options: shell::ProviderDiscoveryConnectionOptionsInput {
                    values: Vec::new(),
                    api_base_path: Some("/v1".to_owned()),
                    timeout_seconds: 30,
                    network_mode: shell::ProviderNetworkModeInput::Public,
                    local_network_approval: None,
                },
                supplied_evidence_ids: Vec::new(),
            },
            shell::SecretProviderCurl::new("curl https://api.example.com/v1/models"),
            &reader,
        ))
        .expect("begin unknown cURL discovery outside Tokio");
        assert_eq!(awaiting.state, "awaiting_more_evidence");
        let progressed =
            tauri::async_runtime::block_on(supply_provider_discovery_curl_evidence_off_runtime(
                &shell,
                awaiting.id,
                awaiting.revision,
                shell::SecretProviderCurl::new(
                    "curl https://api.example.com/v1/chat/completions \
                     -H 'content-type: application/json' \
                     --data '{\"model\":\"synthetic\",\"messages\":[]}'",
                ),
            ))
            .expect("supplemental cURL executes deterministically off runtime");
        assert!(progressed.revision > awaiting.revision);
        assert!(
            !shell
                .list_provider_discovery_evidence(&progressed.id)
                .expect("list supplemental evidence")
                .is_empty()
        );
    }

    #[test]
    fn precommit_capture_supplies_only_the_exact_session_and_restart_requires_recapture() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let session = prepare_precommit_discovery(&shell, "precommit-capture");
        let state = AppState::new(root.path().to_path_buf());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));

        let capture = tauri::async_runtime::block_on(capture_precommit_discovery_credential_with(
            &vault,
            &state,
            &shell,
            &session.id,
            session.revision,
        ))
        .expect("capture process-local discovery credential");
        assert_eq!(capture.clipboard_cleanup, ClipboardCleanupStatus::Cleared);
        let credential = credential_for_discovery_action(
            &state,
            &shell,
            &session,
            session.revision,
            &shell::ContinueProviderDiscoveryActionInput::ApproveCredentialOrigin {
                approval_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            },
        )
        .expect("borrow exact discovery credential")
        .expect("credential-bound action receives a credential");
        assert_eq!(format!("{credential:?}"), "SecretCredential([REDACTED])");
        assert_eq!(*calls.lock().expect("fake calls"), vec!["capture"]);

        let mut unbound = session.clone();
        unbound.credential_binding_requested = false;
        unbound.state = "awaiting_probe_consent".to_owned();
        assert!(
            credential_for_discovery_action(
                &state,
                &shell,
                &unbound,
                unbound.revision,
                &shell::ContinueProviderDiscoveryActionInput::ApproveProbes {
                    approval_id: "00000000-0000-4000-8000-000000000011".to_owned(),
                    approval_grant_sha256: "d".repeat(64),
                },
            )
            .expect("credential-free probes must not consult a credential lease")
            .is_none()
        );

        let restarted = AppState::new(root.path().to_path_buf());
        assert!(
            credential_for_discovery_action(
                &restarted,
                &shell,
                &session,
                session.revision,
                &shell::ContinueProviderDiscoveryActionInput::ApproveCredentialOrigin {
                    approval_id: "00000000-0000-4000-8000-000000000001".to_owned(),
                },
            )
            .is_err(),
            "a process restart must fail closed and require recapture"
        );
    }

    #[tokio::test]
    async fn capture_rejects_exact_bound_slot_appearing_during_clipboard_read() {
        let context = install_context("started");
        let authority = discovery_credential_authority(&context).expect("started authority");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_raw(&context.connection_id);
        vault.insert_bound_during_capture(&context.connection_id, &authority);

        let result = capture_discovery_credential_for_empty_bound_slot_with(
            &vault,
            &context.connection_id,
            &authority,
        )
        .await;
        assert!(
            result.is_err(),
            "the second pre-store observation must reject a newly appeared exact slot"
        );
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec!["vault_bound_status", "capture", "vault_bound_status"]
        );
        assert_eq!(
            vault.raw_status(&context.connection_id),
            CredentialStatus::Available,
            "direct capture must never inspect or mutate the legacy raw slot"
        );
        assert_eq!(
            vault.bound_status(&context.connection_id, &authority),
            CredentialStatus::Available
        );
    }

    #[tokio::test]
    async fn raw_available_exact_missing_handoff_starts_then_stores_once() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let context = install_context("prepared");
        let candidate = commit_candidate(&context);
        let binding = commit_lease_binding(&context);
        state
            .install_discovery_credential_lease(
                binding.clone(),
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_raw(&context.connection_id);
        vault.fail_store_after_effect();
        let journal = FakeDiscoveryJournal::new(context, Arc::clone(&calls));

        assert!(
            promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
                .await
                .expect("authoritative exact postflight wins over ambiguous store return")
        );
        let started = journal.context.lock().expect("fake journal").clone();
        let current_execution_authority =
            discovery_credential_authority(&started).expect("current B execution authority");
        assert_eq!(
            vault.bound_status(&candidate.connection_id, &current_execution_authority),
            CredentialStatus::Available,
            "a mutate-then-error store succeeds only from an exact Match for current B"
        );
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec![
                "wal_context",
                "wal_reserved",
                "vault_bound_status",
                "vault_bound_status",
                "vault_prepare_store",
                "wal_started",
                "vault_store",
                "vault_observe"
            ]
        );
        assert_eq!(
            state.discovery_credential_lease_status(&binding),
            CredentialStatus::Missing,
            "handoff moves the runtime secret exactly once"
        );
        assert_eq!(
            vault.raw_status(&candidate.connection_id),
            CredentialStatus::Available,
            "authority-scoped handoff must preserve the independent raw legacy slot"
        );
        assert!(
            !promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
                .await
                .expect("started handoff replay is a no-op"),
            "a later commit command must not repeat the vault store"
        );
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec![
                "wal_context",
                "wal_reserved",
                "vault_bound_status",
                "vault_bound_status",
                "vault_prepare_store",
                "wal_started",
                "vault_store",
                "vault_observe",
                "wal_context"
            ]
        );
    }

    #[tokio::test]
    async fn discovery_store_recovery_required_never_adopts_visible_match() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let context = install_context("prepared");
        let candidate = commit_candidate(&context);
        state
            .install_discovery_credential_lease(
                commit_lease_binding(&context),
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.require_recovery_after_store_effect();
        let journal = FakeDiscoveryJournal::new(context, Arc::clone(&calls));

        let error = promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
            .await
            .expect_err("visible Match cannot override explicit recovery-required");
        assert_eq!(error.code, "credential_recovery_required");
        assert_eq!(
            journal
                .context
                .lock()
                .expect("fake journal")
                .operation_status,
            "outcome_unknown"
        );
        let calls = calls.lock().expect("fake calls");
        assert_eq!(calls.last(), Some(&"wal_unknown"));
        assert!(
            !calls.contains(&"vault_observe"),
            "durability-unknown WAL settlement must precede and suppress native observation"
        );
    }

    #[tokio::test]
    async fn mismatched_started_discovery_authority_never_reaches_native_store() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let context = install_context("prepared");
        let candidate = commit_candidate(&context);
        state
            .install_discovery_credential_lease(
                commit_lease_binding(&context),
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        let journal = FakeDiscoveryJournal::new(context, Arc::clone(&calls));
        journal.mismatch_next_started_context();

        promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
            .await
            .expect_err("mismatched Started authority must fail before native mutation");
        assert!(
            !calls.lock().expect("fake calls").contains(&"vault_store"),
            "native store is downstream of exact durable Started validation"
        );
    }

    #[tokio::test]
    async fn prepared_store_validation_failure_never_crosses_started_cutpoint() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let context = install_context("prepared");
        let candidate = commit_candidate(&context);
        let binding = commit_lease_binding(&context);
        state
            .install_discovery_credential_lease(
                binding,
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.fail_prepare_store();
        let journal = FakeDiscoveryJournal::new(context, Arc::clone(&calls));

        promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
            .await
            .expect_err("fallible platform preparation must fail before Started");

        let persisted = journal.context.lock().expect("fake journal").clone();
        assert_eq!(persisted.operation_status, "prepared");
        assert!(persisted.native_execution_reservation_id.is_some());
        assert!(persisted.native_execution_id.is_none());
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec![
                "wal_context",
                "wal_reserved",
                "vault_bound_status",
                "vault_bound_status",
                "vault_prepare_store"
            ],
            "no journal start or native store may follow a preparation failure"
        );
    }

    #[tokio::test]
    async fn exact_current_execution_slot_blocks_handoff_before_vault_store() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let context = install_context("prepared");
        let candidate = commit_candidate(&context);
        let binding = commit_lease_binding(&context);
        state
            .install_discovery_credential_lease(
                binding.clone(),
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        let journal = FakeDiscoveryJournal::new(context, Arc::clone(&calls));
        let connection_binding_sha256 = journal
            .context
            .lock()
            .expect("fake journal")
            .connection_binding_sha256
            .clone();
        let current_authority = CredentialAuthority::new(
            journal.next_native_execution_id(),
            connection_binding_sha256,
        )
        .expect("current execution authority");
        vault.insert_bound(&candidate.connection_id, &current_authority);

        promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
            .await
            .expect_err("the exact current physical slot must never be overwritten or adopted");
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec!["wal_context", "wal_reserved", "vault_bound_status"]
        );
        assert_eq!(
            state.discovery_credential_lease_status(&binding),
            CredentialStatus::Available,
            "refusal must not silently discard the recapturable runtime lease"
        );
    }

    #[tokio::test]
    async fn exact_slot_appearing_between_reserved_pre_start_guards_is_never_stored() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let context = install_context("prepared");
        let candidate = commit_candidate(&context);
        let binding = commit_lease_binding(&context);
        state
            .install_discovery_credential_lease(
                binding.clone(),
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        let journal = FakeDiscoveryJournal::new(context, Arc::clone(&calls));
        let authority = CredentialAuthority::new(
            journal.next_native_execution_id(),
            journal
                .context
                .lock()
                .expect("fake journal")
                .connection_binding_sha256
                .clone(),
        )
        .expect("current execution authority");
        vault.insert_bound_after_next_status(&candidate.connection_id, &authority);

        promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
            .await
            .expect_err("the second reserved pre-start status must reject a newly appeared slot");
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec![
                "wal_context",
                "wal_reserved",
                "vault_bound_status",
                "vault_bound_status"
            ]
        );
        assert_eq!(
            journal
                .context
                .lock()
                .expect("fake journal")
                .operation_status,
            "prepared",
            "a failed reservation barrier must remain before the Started store-attempt cutpoint"
        );
        assert_eq!(
            state.discovery_credential_lease_status(&binding),
            CredentialStatus::Available,
            "the runtime lease is not consumed when the second guard rejects"
        );
        assert_eq!(
            vault.bound_status(&candidate.connection_id, &authority),
            CredentialStatus::Available
        );

        let restarted_context = journal.context.lock().expect("fake journal").clone();
        assert!(restarted_context.native_execution_reservation_id.is_some());
        assert!(restarted_context.native_execution_id.is_none());
        require_started_discovery_credential_install(&restarted_context)
            .expect_err("a reserved Prepared operation cannot publish a native slot");
        let reopened = AppState::new(root.path().to_path_buf());
        promote_discovery_credential_lease_with(&vault, &reopened, &journal, &candidate)
            .await
            .expect_err("reopen must not reuse an existing Prepared reservation");
        assert_eq!(
            calls.lock().expect("fake calls").last(),
            Some(&"wal_context"),
            "reopen fails before reserve, platform observation, or store"
        );
        let calls_before_recovery = calls.lock().expect("fake calls").clone();
        let recovery_status =
            discovery_committing_credential_status_with(&vault, &restarted_context)
                .await
                .expect("project crash-after-reserve recovery status");
        assert_eq!(recovery_status, CredentialStatus::Missing);
        assert_eq!(
            *calls.lock().expect("fake calls"),
            calls_before_recovery,
            "bootstrap must not inspect a reserved Prepared physical slot"
        );
        assert_eq!(
            credential_install_recovery_action(true, "prepared", recovery_status)
                .expect("classify cancelled crash after reserve and before Started"),
            CredentialInstallRecoveryAction::DeferToCore,
            "a crash at the reserved-Prepared barrier cannot adopt B"
        );
    }

    #[tokio::test]
    async fn rolled_back_prior_execution_slot_is_not_adopted_by_new_install_execution() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let prior_started = install_context("started");
        let prior_execution_authority =
            discovery_credential_authority(&prior_started).expect("prior execution authority");
        // Simulate a database/context rollback across A's Started cutpoint.
        // The old native item survives, while the restored Prepared operation
        // has neither a reservation nor usable physical authority.
        let mut rolled_back_prepared = prior_started.clone();
        rolled_back_prepared.operation_status = "prepared".to_owned();
        rolled_back_prepared.native_execution_reservation_id = None;
        rolled_back_prepared.native_execution_id = None;
        let candidate = commit_candidate(&rolled_back_prepared);
        let binding = commit_lease_binding(&rolled_back_prepared);
        state
            .install_discovery_credential_lease(
                binding,
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease after restored Prepared snapshot");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(
            &rolled_back_prepared.connection_id,
            &prior_execution_authority,
        );
        vault.restore_rolled_back_bound_slot_before_next_store(
            &rolled_back_prepared.connection_id,
            &prior_execution_authority,
        );
        let journal = FakeDiscoveryJournal::new(rolled_back_prepared, Arc::clone(&calls));

        let result =
            promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate).await;

        assert!(
            result.is_err(),
            "a prior execution envelope restored after a database rollback must not prove the new store"
        );
        let execution_b = journal.context.lock().expect("fake journal").clone();
        let current_execution_authority =
            discovery_credential_authority(&execution_b).expect("current execution B authority");
        assert_ne!(prior_execution_authority, current_execution_authority);
        assert_eq!(
            vault.bound_status(&candidate.connection_id, &prior_execution_authority),
            CredentialStatus::Available,
            "the stale A slot is neither adopted nor overwritten"
        );
        assert_eq!(
            vault.bound_status(&candidate.connection_id, &current_execution_authority),
            CredentialStatus::Missing,
            "a no-effect B store must not be published from stale A evidence"
        );
        assert_eq!(
            credential_install_recovery_action(false, "started", CredentialStatus::Missing)
                .expect("classify crash after Started with no B effect"),
            CredentialInstallRecoveryAction::DeferToCore,
            "bare Started is intent-only and visibility cannot settle it"
        );
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec![
                "wal_context",
                "wal_reserved",
                "vault_bound_status",
                "vault_bound_status",
                "vault_prepare_store",
                "wal_started",
                "vault_store",
                "vault_observe",
                "wal_no_effect"
            ]
        );
    }

    #[tokio::test]
    async fn crash_after_started_before_store_recovers_only_from_exact_b() {
        let mut stale_a = install_context("started");
        stale_a.native_execution_reservation_id = Some("native-execution-A".to_owned());
        stale_a.native_execution_id = Some("native-execution-A".to_owned());
        let stale_authority = discovery_credential_authority(&stale_a).expect("stale A authority");

        let mut current_b = stale_a.clone();
        current_b.native_execution_reservation_id = Some("native-execution-B".to_owned());
        current_b.native_execution_id = Some("native-execution-B".to_owned());
        let current_authority =
            discovery_credential_authority(&current_b).expect("current B authority");
        assert_ne!(stale_authority, current_authority);

        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(&current_b.connection_id, &stale_authority);

        let status = discovery_committing_credential_status_with(&vault, &current_b)
            .await
            .expect("observe current B after crash before store");
        assert_eq!(status, CredentialStatus::Missing);
        assert_eq!(
            credential_install_recovery_action(true, "started", status)
                .expect("classify cancelled exact B no-effect"),
            CredentialInstallRecoveryAction::DeferToCore
        );
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec!["vault_observe"],
            "recovery observes current B once and never falls back to stale A"
        );
        assert_eq!(
            vault.bound_status(&current_b.connection_id, &stale_authority),
            CredentialStatus::Available
        );
        assert_eq!(
            vault.bound_status(&current_b.connection_id, &current_authority),
            CredentialStatus::Missing
        );
    }

    #[tokio::test]
    async fn migrated_pre37_started_without_execution_defers_without_vault_access() {
        let mut legacy_started = install_context("started");
        legacy_started.native_execution_reservation_id = None;
        legacy_started.native_execution_id = None;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));

        let status = discovery_committing_credential_status_with(&vault, &legacy_started)
            .await
            .expect("classify sealed pre37 Started context");
        assert_eq!(status, CredentialStatus::Unreadable);
        assert!(
            calls.lock().expect("fake calls").is_empty(),
            "legacy Started without exact B must not inspect or adopt any vault slot"
        );
        assert_eq!(
            credential_install_recovery_action(true, "started", status)
                .expect("defer cancelled legacy Started recovery to Core"),
            CredentialInstallRecoveryAction::DeferToCore
        );
        require_started_discovery_credential_install(&legacy_started)
            .expect_err("legacy Started cannot produce confirmation authority");
        shell::ProviderDiscoveryCredentialCommitConfirmationDto::try_from(&legacy_started)
            .expect_err("legacy Started cannot forge a native execution confirmation");
    }

    #[test]
    fn migrated_pre37_started_runs_full_adapter_recovery_without_vault_authority() {
        let root = tempdir().expect("temporary root");
        let fixture =
            shell::test_support::seed_synthetic_migrated_pre37_started_discovery(root.path())
                .expect("seed migration-sealed pre37 Started discovery");
        let legacy = fixture
            .shell
            .get_provider_discovery_credential_install_recovery_context(&fixture.session_id)
            .expect("load recovery-only legacy context");
        assert_eq!(legacy.operation_status, "started");
        assert!(legacy.native_execution_reservation_id.is_none());
        assert!(legacy.native_execution_id.is_none());

        let recovered = recover_provider_discovery_credential_installs(&fixture.shell)
            .expect("Tauri startup adapter defers sealed legacy recovery to Core");
        assert_eq!(recovered.len(), 1);
        let unknown = fixture
            .shell
            .get_provider_discovery(&fixture.session_id)
            .expect("load recovered legacy session");
        assert_eq!(unknown.state, "unknown_outcome");
        assert_eq!(unknown.unknown_operation.as_deref(), Some("atomic_commit"));
        assert!(unknown.active_operation_id.is_none());
        assert!(
            fixture
                .shell
                .list_provider_connections()
                .expect("list provider connections")
                .iter()
                .all(|connection| connection.id != legacy.connection_id),
            "sealed pre37 Started recovery cannot publish or adopt a provider graph"
        );
        assert!(
            fixture
                .shell
                .list_provider_discovery_credential_recovery_candidates()
                .expect("list terminal credential recovery candidates")
                .is_empty()
        );
    }

    #[test]
    fn exact_started_discovery_restart_settles_unknown_without_native_observation() {
        let root = tempdir().expect("temporary root");
        let fixture =
            shell::test_support::seed_synthetic_started_discovery_credential_install(root.path())
                .expect("seed exact Started discovery");
        let session = fixture
            .shell
            .get_provider_discovery(&fixture.install.session_id)
            .expect("load Started discovery session");

        settle_started_discovery_credential_recovery(&fixture.shell, &session, &fixture.install)
            .expect("startup settles exact Started as durability unknown");

        let unknown = fixture
            .shell
            .get_provider_discovery(&fixture.install.session_id)
            .expect("reload durability-unknown session");
        assert_eq!(unknown.state, "unknown_outcome");
        assert_eq!(unknown.unknown_operation.as_deref(), Some("atomic_commit"));
        assert!(unknown.active_operation_id.is_none());
        assert!(
            fixture
                .shell
                .list_provider_connections()
                .expect("list connections")
                .iter()
                .all(|connection| connection.id != fixture.install.connection_id),
            "bare Started recovery cannot publish or adopt the provider graph"
        );
    }

    #[tokio::test]
    async fn rolled_back_prepared_wal_never_adopts_future_exact_envelope() {
        let started = install_context("started");
        let stale_authority =
            discovery_credential_authority(&started).expect("stale execution authority");
        let mut context = started;
        context.operation_status = "prepared".to_owned();
        context.native_execution_reservation_id = None;
        context.native_execution_id = None;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(&context.connection_id, &stale_authority);
        assert_eq!(
            discovery_committing_credential_status_with(&vault, &context)
                .await
                .expect("project rollback-visible prepared status"),
            CredentialStatus::Missing,
            "Prepared has no physical authority and cannot inspect or adopt stale A"
        );
        assert!(calls.lock().expect("fake calls").is_empty());
        discovery_credential_authority(&context)
            .expect_err("Prepared cannot invent a physical authority");
        require_started_discovery_credential_install(&context)
            .expect_err("commit confirmation must reject a Prepared WAL");

        let missing_calls = Arc::new(Mutex::new(Vec::new()));
        let missing = FakeDiscoveryVault::new(Arc::clone(&missing_calls));
        assert_eq!(
            discovery_committing_credential_status_with(&missing, &context)
                .await
                .expect("project safe prepared missing status"),
            CredentialStatus::Missing
        );

        let error_calls = Arc::new(Mutex::new(Vec::new()));
        let error = FakeDiscoveryVault::new(Arc::clone(&error_calls));
        error.fail_status();
        assert_eq!(
            discovery_committing_credential_status_with(&error, &context)
                .await
                .expect("Prepared does not consult a nonexistent physical authority"),
            CredentialStatus::Missing
        );
        assert!(error_calls.lock().expect("error calls").is_empty());
    }

    #[tokio::test]
    async fn retry_started_ignores_restored_prior_operation_slot() {
        let mut first = install_context("started");
        first.operation_id = "00000000-0000-4000-8000-000000000041".to_owned();
        first.native_execution_reservation_id = Some("native-execution-A".to_owned());
        first.native_execution_id = Some("native-execution-A".to_owned());
        let mut retry = first.clone();
        retry.operation_id = "00000000-0000-4000-8000-000000000042".to_owned();
        retry.native_execution_reservation_id = Some("native-execution-B".to_owned());
        retry.native_execution_id = Some("native-execution-B".to_owned());
        assert_eq!(first.commit_attempt_id, retry.commit_attempt_id);

        let first_authority =
            discovery_credential_authority(&first).expect("prior operation authority");
        let retry_authority =
            discovery_credential_authority(&retry).expect("retry operation authority");
        assert_ne!(first_authority, retry_authority);

        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(&retry.connection_id, &first_authority);

        // A process restart reconstructs the current authority from durable
        // operation context. Restoring the prior operation's envelope must
        // therefore look missing, never resumable or publishable.
        let reopened_context = retry.clone();
        let stale_status = discovery_committing_credential_status_with(&vault, &reopened_context)
            .await
            .expect("observe retry authority after reopen");
        assert_eq!(stale_status, CredentialStatus::Missing);
        assert_eq!(
            credential_install_recovery_action(false, "started", stale_status)
                .expect("classify restored prior slot"),
            CredentialInstallRecoveryAction::DeferToCore
        );
        assert_eq!(
            vault.bound_status(&retry.connection_id, &first_authority),
            CredentialStatus::Available
        );
        assert_eq!(
            vault.bound_status(&retry.connection_id, &retry_authority),
            CredentialStatus::Missing
        );

        vault.insert_bound(&retry.connection_id, &retry_authority);
        let exact_status = discovery_committing_credential_status_with(&vault, &reopened_context)
            .await
            .expect("observe exact retry authority");
        assert_eq!(exact_status, CredentialStatus::Available);
        assert_eq!(
            credential_install_recovery_action(false, "started", exact_status)
                .expect("classify exact retry slot"),
            CredentialInstallRecoveryAction::DeferToCore
        );
        assert_eq!(
            credential_install_recovery_action(true, "started", exact_status)
                .expect("classify cancelled exact retry slot"),
            CredentialInstallRecoveryAction::DeferToCore
        );
    }

    #[test]
    fn startup_observe_only_leaves_matching_compensation_durable_without_native_effect() {
        let root = tempdir().expect("temporary root");
        let (shell, session, authority) = compensating_started_discovery_fixture(root.path());
        let attempt_id = session
            .commit_attempt_id
            .clone()
            .expect("compensation attempt");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(&session.connection_id, &authority);
        let steps_before = shell
            .list_provider_discovery_compensation_steps(&attempt_id)
            .expect("pending compensation steps");

        tauri::async_runtime::block_on(async {
            let result = drive_provider_discovery_compensation_with(
                &vault,
                &shell,
                session.clone(),
                false,
                CompensationCredentialEffectPolicy::ObserveOnly,
                CompensationObserveErrorPolicy::Defer,
                None,
            )
            .await
            .expect("startup observation is non-mutating");

            match result {
                DiscoveryCompensationDriveResult::Finished(returned) => {
                    assert_eq!(returned, session);
                }
                DiscoveryCompensationDriveResult::NativeConfirmationRequired { .. } => {
                    panic!("bootstrap must never request or synthesize delete authority");
                }
            }
            assert_eq!(*calls.lock().expect("fake calls"), vec!["vault_observe"]);
            assert_eq!(
                vault.bound_status(&session.connection_id, &authority),
                CredentialStatus::Available
            );
            assert_eq!(
                shell
                    .list_provider_discovery_compensation_steps(&attempt_id)
                    .expect("unchanged compensation steps"),
                steps_before
            );
            assert_eq!(
                shell
                    .get_provider_discovery(&session.id)
                    .expect("unchanged compensating session"),
                session
            );
        });
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One end-to-end assertion chain pins both denial and success.
    fn explicit_compensation_rejects_stale_receipt_before_native_delete() {
        let root = tempdir().expect("temporary root");
        let (shell, session, authority) = compensating_started_discovery_fixture(root.path());
        let attempt_id = session
            .commit_attempt_id
            .clone()
            .expect("compensation attempt");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(&session.connection_id, &authority);
        let steps_before = shell
            .list_provider_discovery_compensation_steps(&attempt_id)
            .expect("pending compensation steps");

        tauri::async_runtime::block_on(async {
            let initial = drive_provider_discovery_compensation_with(
                &vault,
                &shell,
                session.clone(),
                false,
                CompensationCredentialEffectPolicy::RequireNativeConfirmation,
                CompensationObserveErrorPolicy::Propagate,
                None,
            )
            .await
            .expect("preflight requests a native receipt without starting");
            let (prompted, exact_context) = match initial {
                DiscoveryCompensationDriveResult::NativeConfirmationRequired {
                    session,
                    context,
                } => (session, context),
                DiscoveryCompensationDriveResult::Finished(_) => {
                    panic!("matching slot must require fresh confirmation")
                }
            };
            assert_eq!(
                shell
                    .list_provider_discovery_compensation_steps(&attempt_id)
                    .expect("pre-prompt step remains pending"),
                steps_before
            );
            assert!(!calls.lock().expect("fake calls").contains(&"vault_delete"));

            let stale_context = NativeCredentialEffectContext::new(
                NativeCredentialEffect::DiscoveryCompensation,
                exact_context.target_id().to_owned(),
                exact_context.origin().to_owned(),
                format!("stale:{}", exact_context.revision()),
            )
            .expect("bounded stale authority context");
            let stale_receipt = vault
                .confirm_compensation(stale_context)
                .await
                .expect("simulate stale native approval");
            drive_provider_discovery_compensation_with(
                &vault,
                &shell,
                prompted.clone(),
                false,
                CompensationCredentialEffectPolicy::RequireNativeConfirmation,
                CompensationObserveErrorPolicy::Propagate,
                Some(stale_receipt),
            )
            .await
            .expect_err("stale authority receipt must fail before durable start or delete");
            assert_eq!(
                shell
                    .list_provider_discovery_compensation_steps(&attempt_id)
                    .expect("stale receipt leaves step pending"),
                steps_before
            );
            assert!(!calls.lock().expect("fake calls").contains(&"vault_delete"));
            assert_eq!(
                vault.bound_status(&session.connection_id, &authority),
                CredentialStatus::Available
            );

            let exact_receipt = vault
                .confirm_compensation(exact_context)
                .await
                .expect("fresh exact native approval");
            let completed = drive_provider_discovery_compensation_with(
                &vault,
                &shell,
                prompted,
                false,
                CompensationCredentialEffectPolicy::RequireNativeConfirmation,
                CompensationObserveErrorPolicy::Propagate,
                Some(exact_receipt),
            )
            .await
            .expect("fresh exact receipt permits one exact delete");
            assert!(matches!(
                completed,
                DiscoveryCompensationDriveResult::Finished(_)
            ));
            assert_eq!(
                vault.bound_status(&session.connection_id, &authority),
                CredentialStatus::Missing
            );
            assert_eq!(
                calls
                    .lock()
                    .expect("fake calls")
                    .iter()
                    .filter(|call| **call == "vault_delete")
                    .count(),
                1
            );
            let steps_after = shell
                .list_provider_discovery_compensation_steps(&attempt_id)
                .expect("completed compensation steps");
            assert!(
                steps_after
                    .iter()
                    .find(|step| step.kind == "remove_credential_slot")
                    .is_some_and(|step| step.status == "completed")
            );
        });
    }

    #[tokio::test]
    async fn compensation_deletes_only_the_producing_operation_slot() {
        let mut prior = install_context("started");
        prior.operation_id = "00000000-0000-4000-8000-000000000051".to_owned();
        prior.native_execution_reservation_id = Some("native-execution-A".to_owned());
        prior.native_execution_id = Some("native-execution-A".to_owned());
        let mut producing = prior.clone();
        producing.operation_id = "00000000-0000-4000-8000-000000000052".to_owned();
        producing.native_execution_reservation_id = Some("native-execution-B".to_owned());
        producing.native_execution_id = Some("native-execution-B".to_owned());
        assert_eq!(prior.commit_attempt_id, producing.commit_attempt_id);

        let prior_authority =
            discovery_credential_authority(&prior).expect("prior operation authority");
        let compensation_context = shell::ProviderDiscoveryCredentialAuthorityDto {
            operation_id: producing.operation_id.clone(),
            native_execution_id: producing
                .native_execution_id
                .clone()
                .expect("producing native execution authority"),
            commit_attempt_id: producing.commit_attempt_id.clone(),
            connection_id: producing.connection_id.clone(),
            credential_api_origin: "https://api.example".to_owned(),
            credential_origin_approval_id: "00000000-0000-4000-8000-000000000053".to_owned(),
            credential_origin_grant_sha256: "c".repeat(64),
            connection_binding_sha256: producing.connection_binding_sha256.clone(),
        };
        let producing_authority =
            discovery_compensation_credential_authority(&compensation_context)
                .expect("producing operation compensation authority");
        assert_eq!(
            producing_authority.authority_id(),
            producing
                .native_execution_id
                .as_deref()
                .expect("producing native execution")
        );
        assert_ne!(
            producing_authority.authority_id(),
            producing.commit_attempt_id.as_str(),
            "the reusable attempt ID must not select a physical compensation slot"
        );
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(&producing.connection_id, &prior_authority);
        vault.insert_bound(&producing.connection_id, &producing_authority);

        assert_eq!(
            observe_discovery_compensation_slot(
                &vault,
                &producing.connection_id,
                &producing_authority,
                CompensationObserveErrorPolicy::Propagate,
            )
            .await
            .expect("observe producing slot"),
            Some(BoundCredentialObservation::Match)
        );
        let (deleted, postflight) = delete_and_observe_discovery_bound_slot(
            &vault,
            &producing.connection_id,
            &producing_authority,
        )
        .await;
        deleted.expect("delete producing slot");
        assert_eq!(postflight, Ok(BoundCredentialObservation::Missing));
        assert_eq!(
            vault.bound_status(&producing.connection_id, &producing_authority),
            CredentialStatus::Missing
        );
        assert_eq!(
            vault.bound_status(&producing.connection_id, &prior_authority),
            CredentialStatus::Available,
            "compensation must not delete a prior retry operation's physical slot"
        );
    }

    #[tokio::test]
    async fn compensation_observe_error_defers_one_slot_while_another_advances() {
        let context = install_context("started");
        let authority = discovery_credential_authority(&context).expect("started authority");
        let blocked_calls = Arc::new(Mutex::new(Vec::new()));
        let blocked = FakeDiscoveryVault::new(Arc::clone(&blocked_calls));
        blocked.insert_raw(&context.connection_id);
        blocked.insert_bound(&context.connection_id, &authority);
        blocked.fail_observe();

        assert_eq!(
            observe_discovery_compensation_slot(
                &blocked,
                &context.connection_id,
                &authority,
                CompensationObserveErrorPolicy::Defer,
            )
            .await
            .expect("startup observation errors are deferred"),
            None,
            "a backend read error defers this pending compensation without claiming it"
        );
        assert!(
            observe_discovery_compensation_slot(
                &blocked,
                &context.connection_id,
                &authority,
                CompensationObserveErrorPolicy::Propagate,
            )
            .await
            .is_err(),
            "an explicit compensation command still surfaces the platform error"
        );
        assert_eq!(
            *blocked_calls.lock().expect("blocked calls"),
            vec!["vault_observe", "vault_observe"]
        );
        assert_eq!(
            blocked.bound_status(&context.connection_id, &authority),
            CredentialStatus::Available,
            "the exact slot remains retryable and no delete was attempted"
        );

        let ready_calls = Arc::new(Mutex::new(Vec::new()));
        let ready = FakeDiscoveryVault::new(Arc::clone(&ready_calls));
        ready.insert_raw(&context.connection_id);
        ready.insert_bound(&context.connection_id, &authority);
        ready.fail_delete_after_effect();
        assert_eq!(
            observe_discovery_compensation_slot(
                &ready,
                &context.connection_id,
                &authority,
                CompensationObserveErrorPolicy::Defer,
            )
            .await
            .expect("ready recovery observation"),
            Some(BoundCredentialObservation::Match),
            "a later recovery candidate can still advance"
        );
        let (delete_result, postflight) =
            delete_and_observe_discovery_bound_slot(&ready, &context.connection_id, &authority)
                .await;
        assert!(delete_result.is_err(), "simulate lost delete response");
        assert_eq!(
            credential_compensation_delete_outcome(&delete_result, &postflight),
            CredentialCompensationDeleteOutcome::Complete,
            "authoritative Missing postflight completes despite mutate-then-error"
        );
        assert_eq!(
            ready.raw_status(&context.connection_id),
            CredentialStatus::Available,
            "exact compensation deletion preserves the raw legacy sentinel"
        );

        blocked.restore_observe();
        assert_eq!(
            observe_discovery_compensation_slot(
                &blocked,
                &context.connection_id,
                &authority,
                CompensationObserveErrorPolicy::Defer,
            )
            .await
            .expect("retry deferred observation"),
            Some(BoundCredentialObservation::Match)
        );
        let (delete_result, postflight) =
            delete_and_observe_discovery_bound_slot(&blocked, &context.connection_id, &authority)
                .await;
        delete_result.expect("retry exact deferred slot");
        assert_eq!(postflight, Ok(BoundCredentialObservation::Missing));
        assert_eq!(
            blocked.raw_status(&context.connection_id),
            CredentialStatus::Available
        );
    }

    #[test]
    fn discovery_compensation_recovery_required_never_accepts_visible_missing() {
        let delete_result = Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
        let postflight = Ok(BoundCredentialObservation::Missing);

        assert_eq!(
            credential_compensation_delete_outcome(&delete_result, &postflight),
            CredentialCompensationDeleteOutcome::Unknown,
            "explicit recovery-required must outrank immediate visibility"
        );
    }

    #[test]
    fn provider_curl_ingress_is_nonempty_and_bounded() {
        assert!(bounded_secret_curl(String::new()).is_err());
        assert!(bounded_secret_curl(" \n".to_owned()).is_err());
        assert!(bounded_secret_curl("curl https://example.test".to_owned()).is_ok());
        assert!(bounded_secret_curl("x".repeat(MAXIMUM_PROVIDER_CURL_BYTES + 1)).is_err());
    }

    #[test]
    fn credential_install_recovery_requires_started_wal_provenance() {
        for cancellation_pending in [false, true] {
            for credential_status in [
                CredentialStatus::Missing,
                CredentialStatus::Available,
                CredentialStatus::Unreadable,
            ] {
                assert_eq!(
                    credential_install_recovery_action(
                        cancellation_pending,
                        "prepared",
                        credential_status,
                    )
                    .expect("prepared recovery state"),
                    CredentialInstallRecoveryAction::DeferToCore,
                    "a native effect cannot be inferred before the durable started marker"
                );

                let expected = CredentialInstallRecoveryAction::DeferToCore;
                assert_eq!(
                    credential_install_recovery_action(
                        cancellation_pending,
                        "started",
                        credential_status,
                    )
                    .expect("started recovery state"),
                    expected
                );
            }
        }

        for cancellation_pending in [false, true] {
            for operation_status in ["", "completed", "unknown_outcome"] {
                for credential_status in [
                    CredentialStatus::Missing,
                    CredentialStatus::Available,
                    CredentialStatus::Unreadable,
                ] {
                    assert!(
                        credential_install_recovery_action(
                            cancellation_pending,
                            operation_status,
                            credential_status,
                        )
                        .is_err(),
                        "an unrecognized WAL status must fail closed"
                    );
                }
            }
        }
    }

    #[test]
    fn unreadable_recovery_observation_defers_to_core_without_aborting_bootstrap() {
        let status = status_only_bound_observation(Err(PlatformError::new(
            PlatformErrorCode::StorageUnavailable,
        )));
        assert_eq!(status, CredentialStatus::Unreadable);
        assert_eq!(
            credential_install_recovery_action(false, "started", status)
                .expect("unreadable started recovery classification"),
            CredentialInstallRecoveryAction::DeferToCore
        );

        assert_eq!(
            status_only_bound_observation(Ok(BoundCredentialObservation::Match)),
            CredentialStatus::Available,
            "a legitimate exact envelope remains resumable"
        );
    }

    #[test]
    fn assistant_turn_request_rejects_renderer_estimates() {
        let request = json!({
            "session_id": "synthetic-session",
            "estimate": {
                "input_tokens": 1,
                "maximum_output_tokens": 1,
                "maximum_cost_micro_units": 0
            }
        });

        serde_json::from_value::<ProviderDiscoverySessionRequest>(request)
            .expect_err("renderer-authored estimates must not cross Tauri IPC");
    }

    #[test]
    fn session_scoped_outbox_poll_request_is_exact_and_bounded_by_rust() {
        let request =
            serde_json::from_value::<PollProviderDiscoveryEventsForSessionRequest>(json!({
                "session_id": "selected-session",
                "limit": 100
            }))
            .expect("decode session-scoped poll request");
        assert_eq!(request.session_id, "selected-session");
        assert_eq!(request.limit, 100);

        serde_json::from_value::<PollProviderDiscoveryEventsForSessionRequest>(json!({
            "limit": 100
        }))
        .expect_err("session-scoped polling requires a session id");
        serde_json::from_value::<PollProviderDiscoveryEventsForSessionRequest>(json!({
            "session_id": "selected-session",
            "limit": 100,
            "acknowledge_foreign_sessions": true
        }))
        .expect_err("the WebView cannot request foreign-session acknowledgement");
    }

    #[test]
    fn assistant_turn_fails_closed_without_application_or_platform_state() {
        let error = run_provider_discovery_assistant_turn(ProviderDiscoverySessionRequest {
            session_id: "synthetic-session".to_owned(),
        })
        .expect_err("remote assistant execution must remain unavailable");

        assert_eq!(error.code, "assistant_pricing_unavailable");
        assert_eq!(
            error.message_key,
            "provider.discovery.assistant_pricing_unavailable"
        );
        assert!(!error.recoverable);
    }

    fn prepare_precommit_discovery(
        shell: &shell::ShellApi,
        connection_id: &str,
    ) -> shell::ProviderDiscoverySessionDto {
        let selecting = shell
            .begin_provider_discovery(shell::BeginProviderDiscoveryInput {
                connection_id: connection_id.to_owned(),
                display_name: "Synthetic precommit discovery".to_owned(),
                site_url: "https://openrouter.ai/".to_owned(),
                docs_url: None,
                credential_binding_requested: true,
                preferred_assistant: None,
                connection_options: shell::ProviderDiscoveryConnectionOptionsInput {
                    values: Vec::new(),
                    api_base_path: None,
                    timeout_seconds: 30,
                    network_mode: shell::ProviderNetworkModeInput::Public,
                    local_network_approval: None,
                },
                supplied_evidence_ids: Vec::new(),
                source: shell::BeginProviderDiscoverySourceInput::KnownProvider {
                    template_id: "openrouter-v1".to_owned(),
                },
            })
            .expect("begin synthetic discovery");
        let candidate = shell
            .list_provider_discovery_candidates(&selecting.id)
            .expect("list discovery candidates")
            .into_iter()
            .find(|candidate| {
                matches!(
                    &candidate.summary,
                    shell::DiscoveryCandidateSummaryDto::ProviderTemplate { template_id, .. }
                        if template_id == "openrouter-v1"
                )
            })
            .expect("OpenRouter candidate");
        let approval = shell
            .continue_provider_discovery(
                shell::ContinueProviderDiscoveryInput {
                    session_id: selecting.id,
                    action_id: "00000000-0000-4000-8000-000000000010".to_owned(),
                    expected_revision: selecting.revision,
                    action: shell::ContinueProviderDiscoveryActionInput::SelectTemplate {
                        candidate_id: candidate.id,
                    },
                },
                None,
            )
            .expect("select credential-bound template");
        assert_eq!(approval.state, "awaiting_credential_origin_approval");
        approval
    }

    #[test]
    fn discovery_cancellation_transition_does_not_wait_for_global_credential_lock() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let awaiting = prepare_precommit_discovery(&shell, "prompt-cancellation");
        let state = AppState::new(root.path().to_path_buf());
        let _operation = tauri::async_runtime::block_on(state.lock_provider_credential_operation());

        let cancelled = request_provider_discovery_cancellation(
            &state,
            &shell,
            &CancelProviderDiscoveryRequest {
                session_id: awaiting.id,
                expected_revision: awaiting.revision,
            },
        )
        .expect("durable cancellation transition must not wait for the credential gate");

        assert_eq!(cancelled.state, "cancelled");
    }

    #[test]
    fn accepted_discovery_cancellation_revokes_the_registered_authenticated_request() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let awaiting = prepare_precommit_discovery(&shell, "registered-cancellation");
        let state = AppState::new(root.path().to_path_buf());
        let (_registration, cancelled) =
            register_active_discovery_request(&awaiting.id).expect("register active request");
        assert!(!*cancelled.borrow());

        let session = request_provider_discovery_cancellation(
            &state,
            &shell,
            &CancelProviderDiscoveryRequest {
                session_id: awaiting.id,
                expected_revision: awaiting.revision,
            },
        )
        .expect("accept cancellation");

        assert_eq!(session.state, "cancelled");
        assert!(
            *cancelled.borrow(),
            "an accepted cancellation must revoke the authenticated dispatch token"
        );
    }

    fn credential_connection_input(
        shell: &shell::ShellApi,
        connection_id: &str,
    ) -> shell::CreateProviderConnectionInput {
        let template = shell
            .list_provider_templates()
            .expect("list provider templates")
            .into_iter()
            .find(|template| template.id == "openrouter-v1")
            .expect("OpenRouter credential-bound public template");
        let origin = template.default_api_origin.expect("template origin");
        shell::CreateProviderConnectionInput {
            id: connection_id.to_owned(),
            template_id: template.id,
            template_version: template.manifest_version,
            display_name: format!("Synthetic {connection_id}"),
            api_origin: origin.clone(),
            api_base_path: None,
            network_mode: shell::ProviderNetworkModeInput::Public,
            local_network_approval: None,
            values: Vec::new(),
            approved_credential_origin: Some(origin),
            timeout_seconds: 30,
        }
    }

    fn local_model_sync_connection_input(
        shell: &shell::ShellApi,
        connection_id: &str,
        origin: &str,
    ) -> shell::CreateProviderConnectionInput {
        let template = shell
            .list_provider_templates()
            .expect("list provider templates")
            .into_iter()
            .find(|template| template.id == "openai-chat-compatible-v1")
            .expect("OpenAI-compatible model-list template");
        shell::CreateProviderConnectionInput {
            id: connection_id.to_owned(),
            template_id: template.id,
            template_version: template.manifest_version,
            display_name: "Synthetic leased model sync".to_owned(),
            api_origin: origin.to_owned(),
            api_base_path: Some("/v1".to_owned()),
            network_mode: shell::ProviderNetworkModeInput::LocalLoopback,
            local_network_approval: None,
            values: vec![shell::ConnectionConfigEntryDto {
                key: "api_base_url".to_owned(),
                value: shell::ConnectionConfigValueDto::Text(format!("{origin}/v1")),
            }],
            approved_credential_origin: Some(origin.to_owned()),
            timeout_seconds: 5,
        }
    }

    fn spawn_blocking_model_list_provider() -> (
        String,
        mpsc::Receiver<String>,
        mpsc::Sender<()>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind model-list provider");
        let address = listener.local_addr().expect("model-list provider address");
        let (request_sender, request_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept model-list request");
            let request = read_http_headers(&mut stream);
            request_sender
                .send(request)
                .expect("report model-list request");
            release_receiver
                .recv()
                .expect("release model-list response");
            let body = r#"{"data":[{"id":"leased-model"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write model-list response");
        });
        (
            format!("http://{address}"),
            request_receiver,
            release_sender,
            handle,
        )
    }

    fn read_http_headers(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("model-list request timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        loop {
            let read = stream.read(&mut buffer).expect("read model-list request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8(request).expect("model-list request is UTF-8")
    }

    fn credentialless_connection_input(
        shell: &shell::ShellApi,
        connection_id: &str,
    ) -> shell::CreateProviderConnectionInput {
        let template = shell
            .list_provider_templates()
            .expect("list provider templates")
            .into_iter()
            .find(|template| !template.credential_required && template.default_api_origin.is_some())
            .expect("credentialless template with a default origin");
        let origin = template.default_api_origin.expect("template origin");
        let network_mode = match template.default_network_mode.as_str() {
            "public" => shell::ProviderNetworkModeInput::Public,
            "local_loopback" => shell::ProviderNetworkModeInput::LocalLoopback,
            other => panic!("unexpected credentialless network mode: {other}"),
        };
        shell::CreateProviderConnectionInput {
            id: connection_id.to_owned(),
            template_id: template.id,
            template_version: template.manifest_version,
            display_name: format!("Synthetic {connection_id}"),
            api_origin: origin,
            api_base_path: None,
            network_mode,
            local_network_approval: None,
            values: Vec::new(),
            approved_credential_origin: None,
            timeout_seconds: 30,
        }
    }

    fn install_provider_credential(
        shell: &shell::ShellApi,
        connection_id: &str,
    ) -> shell::ProviderCredentialAccessAuthorityContext {
        let authority = shell
            .propose_provider_credential_install_authority(connection_id)
            .expect("propose test credential install authority");
        let install = shell
            .prepare_provider_credential_install_operation(
                connection_id,
                &authority,
                shell::ProviderCredentialSlotStatusInput::Missing,
            )
            .expect("prepare test credential install");
        shell
            .start_provider_credential_operation(&install.operation_id, &install.plan_sha256)
            .expect("start test credential install");
        shell
            .finish_provider_credential_operation(
                &install.operation_id,
                &install.plan_sha256,
                shell::ProviderCredentialSlotStatusInput::Available,
            )
            .expect("finish test credential install");
        shell
            .ensure_provider_credential_access_settled(connection_id)
            .expect("read installed credential authority")
    }

    fn remove_provider_credential(shell: &shell::ShellApi, connection_id: &str) {
        let removal = shell
            .prepare_provider_credential_operation(
                connection_id,
                shell::ProviderCredentialOperationKindInput::RemoveCredential,
                shell::ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare test credential removal");
        shell
            .start_provider_credential_operation(&removal.operation_id, &removal.plan_sha256)
            .expect("start test credential removal");
        shell
            .finish_provider_credential_operation(
                &removal.operation_id,
                &removal.plan_sha256,
                shell::ProviderCredentialSlotStatusInput::Missing,
            )
            .expect("finish test credential removal");
    }

    fn discovery_input(
        connection: &shell::ProviderConnectionDto,
        source: shell::BeginProviderDiscoverySourceInput,
    ) -> shell::BeginProviderDiscoveryInput {
        shell::BeginProviderDiscoveryInput {
            connection_id: connection.id.clone(),
            display_name: format!("Discovery {}", connection.id),
            site_url: connection.api_origin.clone(),
            docs_url: None,
            credential_binding_requested: connection.credential_binding_required,
            preferred_assistant: None,
            connection_options: shell::ProviderDiscoveryConnectionOptionsInput {
                values: Vec::new(),
                api_base_path: connection.api_base_path.clone(),
                timeout_seconds: connection.timeout_seconds,
                network_mode: match connection.network_mode.as_str() {
                    "public" => shell::ProviderNetworkModeInput::Public,
                    "local_loopback" => shell::ProviderNetworkModeInput::LocalLoopback,
                    other => panic!("unexpected discovery network mode: {other}"),
                },
                local_network_approval: None,
            },
            supplied_evidence_ids: Vec::new(),
            source,
        }
    }

    fn discovery_curl_input(
        connection: &shell::ProviderConnectionDto,
    ) -> shell::BeginProviderDiscoveryCurlInput {
        shell::BeginProviderDiscoveryCurlInput {
            connection_id: connection.id.clone(),
            display_name: format!("cURL discovery {}", connection.id),
            docs_url: None,
            credential_binding_requested: connection.credential_binding_required,
            preferred_assistant: None,
            connection_options: shell::ProviderDiscoveryConnectionOptionsInput {
                values: Vec::new(),
                api_base_path: connection.api_base_path.clone(),
                timeout_seconds: connection.timeout_seconds,
                network_mode: match connection.network_mode.as_str() {
                    "public" => shell::ProviderNetworkModeInput::Public,
                    "local_loopback" => shell::ProviderNetworkModeInput::LocalLoopback,
                    other => panic!("unexpected discovery network mode: {other}"),
                },
                local_network_approval: None,
            },
            supplied_evidence_ids: Vec::new(),
        }
    }

    fn install_context(
        operation_status: &str,
    ) -> shell::ProviderDiscoveryCredentialInstallContextDto {
        let native_execution_id =
            (operation_status == "started").then(|| Uuid::new_v4().to_string());
        shell::ProviderDiscoveryCredentialInstallContextDto {
            session_id: "handoff-session".to_owned(),
            session_revision: 9,
            operation_id: "handoff-operation".to_owned(),
            operation_status: operation_status.to_owned(),
            native_execution_reservation_id: native_execution_id.clone(),
            native_execution_id,
            commit_attempt_id: "00000000-0000-4000-8000-000000000020".to_owned(),
            commit_plan_sha256: "a".repeat(64),
            commit_phase: "prepared".to_owned(),
            connection_id: "handoff-connection".to_owned(),
            connection_binding_sha256: "b".repeat(64),
        }
    }

    fn commit_candidate(
        context: &shell::ProviderDiscoveryCredentialInstallContextDto,
    ) -> DiscoveryCredentialCommitCandidate {
        DiscoveryCredentialCommitCandidate {
            session_id: context.session_id.clone(),
            session_revision: context.session_revision,
            connection_id: context.connection_id.clone(),
            commit_attempt_id: context.commit_attempt_id.clone(),
            commit_plan_sha256: context.commit_plan_sha256.clone(),
        }
    }

    fn commit_lease_binding(
        context: &shell::ProviderDiscoveryCredentialInstallContextDto,
    ) -> DiscoveryCredentialLeaseBinding {
        DiscoveryCredentialLeaseBinding {
            session_id: context.session_id.clone(),
            connection_id: context.connection_id.clone(),
            credential_origin_approval_id: "00000000-0000-4000-8000-000000000030".to_owned(),
            credential_origin_grant_sha256: "c".repeat(64),
            connection_binding_sha256: context.connection_binding_sha256.clone(),
        }
    }
}
