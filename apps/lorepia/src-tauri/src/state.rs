use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use lorepia_shell_api::{
    BootstrapDto, ChatEventStream, ClaimedInteractionEffect, ProviderCatalogImportPlanDto,
    ProviderCatalogImportResultDto, SecretCredential, ShellApi, SignedCatalogEnvelope,
    TaskCredentialRead, TaskCredentialReader,
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_lorepia_platform::{CredentialStatus, NativeCredential, StagedImport};
use tokio::sync::{
    Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard, OwnedMutexGuard, OwnedRwLockReadGuard,
    RwLock as AsyncRwLock, RwLockWriteGuard as AsyncRwLockWriteGuard, oneshot, watch,
};
use uuid::Uuid;

use crate::{
    contract::{
        InteractionEffectEventDto, MemorySupervisorPhaseDto, MemorySupervisorStatusDto,
        SubscribeGenerationRequest,
    },
    error::{CommandError, CommandResult},
};

const MAXIMUM_IMPORT_TICKETS: usize = 16;
const MAXIMUM_CATALOG_TICKETS: usize = 4;
const MAXIMUM_DISCOVERY_CREDENTIAL_LEASES: usize = 16;
const MAXIMUM_CHAT_STREAMS: usize = 32;
const MEMORY_SUPERVISOR_IDLE_POLL: Duration = Duration::from_millis(500);
const INTERACTION_SUPERVISOR_IDLE_POLL: Duration = Duration::from_millis(500);
const LIFECYCLE_SUPERVISOR_IDLE_POLL: Duration = Duration::from_millis(500);
const LIFECYCLE_SUPERVISOR_BATCH_SIZE: u32 = 64;
const MAXIMUM_PENDING_INTERACTION_DELIVERIES: usize = 128;
const MAXIMUM_COMPLETED_INTERACTION_DELIVERIES: usize = 256;
pub(crate) const MEMORY_SUPERVISOR_STATUS_EVENT: &str = "memory-supervisor-status";
pub(crate) const INTERACTION_EFFECT_EVENT: &str = "interaction-effect";

pub struct AppState {
    data_root: PathBuf,
    app: Option<AppHandle>,
    shell: Mutex<Option<ShellApi>>,
    startup: AsyncMutex<()>,
    ready: AtomicBool,
    import_tickets: Arc<Mutex<TicketStore<StagedImport>>>,
    catalog_tickets: Mutex<TicketStore<CatalogImportTicket>>,
    chat_streams: Arc<ChatStreamRegistry>,
    memory_supervisor_shutdown: Mutex<Option<watch::Sender<bool>>>,
    memory_supervisor_status: Arc<Mutex<MemorySupervisorStatusDto>>,
    interaction_supervisor_shutdown: Mutex<Option<watch::Sender<bool>>>,
    interaction_deliveries: Arc<Mutex<InteractionDeliveryRegistry>>,
    lifecycle_supervisor_shutdown: Mutex<Option<watch::Sender<bool>>>,
    provider_credential_operation: Arc<AsyncRwLock<()>>,
    legacy_credential_admission: Arc<AsyncMutex<()>>,
    discovery_credential_leases: Mutex<DiscoveryCredentialLeaseRegistry>,
}

pub struct CatalogImportTicket {
    pub plan: ProviderCatalogImportPlanDto,
    pub envelope: SignedCatalogEnvelope,
}

struct TicketStore<T> {
    values: HashMap<String, T>,
    reservations: HashSet<String>,
    capacity: usize,
}

/// Rust-only binding for one process-local discovery credential.
///
/// It intentionally has no serde surface. The stable approval and connection
/// hashes are issued by Core, so a renderer-selected session identifier cannot
/// move a credential to another origin, auth scheme, or connection draft.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct DiscoveryCredentialLeaseBinding {
    pub(crate) session_id: String,
    pub(crate) connection_id: String,
    pub(crate) credential_origin_approval_id: String,
    pub(crate) credential_origin_grant_sha256: String,
    pub(crate) connection_binding_sha256: String,
}

struct DiscoveryCredentialLease {
    binding: DiscoveryCredentialLeaseBinding,
    credential: NativeCredential,
}

struct DiscoveryCredentialLeaseRegistry {
    values: HashMap<String, DiscoveryCredentialLease>,
    capacity: usize,
}

pub(crate) struct TicketReservation<T> {
    ticket_id: String,
    value: Option<T>,
    store: Arc<Mutex<TicketStore<T>>>,
}

struct ChatStreamRegistry {
    slots: Mutex<HashMap<String, ChatStreamSlot>>,
    capacity: usize,
}

struct ChatStreamSlot {
    marker: Arc<()>,
    dispose: Option<oneshot::Sender<()>>,
}

pub(crate) struct PlatformTaskCredentialReader {
    pub(crate) app: AppHandle,
    pub(crate) shell: ShellApi,
    pub(crate) inherited_dispatch_lease: Option<lorepia_shell_api::TaskCredentialLease>,
}

struct InteractionDeliveryRegistry {
    pending: HashMap<String, ClaimedInteractionEffect>,
    effect_deliveries: HashMap<String, String>,
    completed: HashSet<String>,
    completed_order: VecDeque<String>,
}

impl TaskCredentialReader for PlatformTaskCredentialReader {
    fn credential_for<'a>(
        &'a self,
        connection_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = TaskCredentialRead> + Send + 'a>> {
        Box::pin(async move {
            let Some(state) = self.app.try_state::<AppState>() else {
                return TaskCredentialRead::Unreadable;
            };
            let dispatch_lease =
                provider_dispatch_lease(&state, self.inherited_dispatch_lease.as_ref()).await;
            let connection = match self.shell.list_provider_connections() {
                Ok(connections) => connections
                    .into_iter()
                    .find(|candidate| candidate.id == connection_id),
                Err(_) => return TaskCredentialRead::Unreadable,
            };
            let Some(connection) = connection else {
                return TaskCredentialRead::Unreadable;
            };
            if !connection.credential_binding_required {
                return TaskCredentialRead::MissingWithLease(dispatch_lease);
            }
            let read = crate::credential_operations::read_provider_connection_credential(
                &self.app,
                &self.shell,
                connection_id,
            )
            .await
            .map(|read| read.credential);
            task_credential_read_with_lease(read, dispatch_lease)
        })
    }
}

async fn provider_dispatch_lease(
    state: &AppState,
    inherited: Option<&lorepia_shell_api::TaskCredentialLease>,
) -> lorepia_shell_api::TaskCredentialLease {
    match inherited {
        Some(lease) => lease.clone(),
        None => lorepia_shell_api::TaskCredentialLease::new(
            state.lease_provider_credential_operation().await,
        ),
    }
}

fn task_credential_read_with_lease(
    read: CommandResult<Option<NativeCredential>>,
    dispatch_lease: lorepia_shell_api::TaskCredentialLease,
) -> TaskCredentialRead {
    match read {
        Ok(Some(value)) => TaskCredentialRead::AvailableWithLease {
            credential: SecretCredential::new(value.into_secret_string()),
            lease: dispatch_lease,
        },
        Ok(None) => TaskCredentialRead::MissingWithLease(dispatch_lease),
        Err(_) => TaskCredentialRead::Unreadable,
    }
}

impl InteractionDeliveryRegistry {
    fn new() -> Self {
        Self {
            pending: HashMap::new(),
            effect_deliveries: HashMap::new(),
            completed: HashSet::new(),
            completed_order: VecDeque::new(),
        }
    }

    fn register(
        &mut self,
        claim: ClaimedInteractionEffect,
    ) -> Result<InteractionEffectEventDto, Box<ClaimedInteractionEffect>> {
        if let Some(previous_delivery) = self.effect_deliveries.remove(&claim.delivery.effect_id) {
            self.pending.remove(&previous_delivery);
        }
        if self.pending.len() >= MAXIMUM_PENDING_INTERACTION_DELIVERIES {
            return Err(Box::new(claim));
        }
        let delivery_id = Uuid::new_v4().to_string();
        let event = InteractionEffectEventDto {
            delivery_id: delivery_id.clone(),
            effect_id: claim.delivery.effect_id.clone(),
            conversation_id: claim.delivery.conversation_id.clone(),
            branch_id: claim.delivery.branch_id.clone(),
            resulting_state_revision: claim.delivery.resulting_state_revision,
            event_created_at: claim.delivery.event_created_at.clone(),
            effect: claim.delivery.effect.clone(),
        };
        self.effect_deliveries
            .insert(claim.delivery.effect_id.clone(), delivery_id.clone());
        self.pending.insert(delivery_id, claim);
        Ok(event)
    }

    fn pending_events(&self) -> Vec<InteractionEffectEventDto> {
        let mut values = self
            .pending
            .iter()
            .map(|(delivery_id, claim)| InteractionEffectEventDto {
                delivery_id: delivery_id.clone(),
                effect_id: claim.delivery.effect_id.clone(),
                conversation_id: claim.delivery.conversation_id.clone(),
                branch_id: claim.delivery.branch_id.clone(),
                resulting_state_revision: claim.delivery.resulting_state_revision,
                event_created_at: claim.delivery.event_created_at.clone(),
                effect: claim.delivery.effect.clone(),
            })
            .collect::<Vec<_>>();
        values.sort_by(|left, right| {
            left.effect_id
                .cmp(&right.effect_id)
                .then_with(|| left.delivery_id.cmp(&right.delivery_id))
        });
        values
    }

    fn is_completed(&self, delivery_id: &str) -> bool {
        self.completed.contains(delivery_id)
    }

    fn complete(&mut self, delivery_id: &str) {
        let Some(claim) = self.pending.remove(delivery_id) else {
            return;
        };
        if self
            .effect_deliveries
            .get(&claim.delivery.effect_id)
            .is_some_and(|current| current == delivery_id)
        {
            self.effect_deliveries.remove(&claim.delivery.effect_id);
        }
        if self.completed.insert(delivery_id.to_owned()) {
            self.completed_order.push_back(delivery_id.to_owned());
        }
        while self.completed_order.len() > MAXIMUM_COMPLETED_INTERACTION_DELIVERIES {
            if let Some(expired) = self.completed_order.pop_front() {
                self.completed.remove(&expired);
            }
        }
    }

    fn remove(&mut self, delivery_id: &str) -> Option<ClaimedInteractionEffect> {
        let claim = self.pending.remove(delivery_id)?;
        if self
            .effect_deliveries
            .get(&claim.delivery.effect_id)
            .is_some_and(|current| current == delivery_id)
        {
            self.effect_deliveries.remove(&claim.delivery.effect_id);
        }
        Some(claim)
    }
}

/// Owns one bounded renderer subscription without owning its Core generation.
///
/// Dropping this value unregisters only the forwarding receiver. Explicit Core
/// cancellation remains a separate command.
pub(crate) struct ChatStreamRegistration {
    stream_id: String,
    marker: Arc<()>,
    dispose: oneshot::Receiver<()>,
    registry: Arc<ChatStreamRegistry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TicketInsertError {
    Duplicate,
    Busy,
}

impl<T> TicketStore<T> {
    fn new(capacity: usize) -> Self {
        Self {
            values: HashMap::new(),
            reservations: HashSet::new(),
            capacity,
        }
    }

    fn insert(&mut self, id: String, value: T) -> Result<(), TicketInsertError> {
        if self.values.contains_key(&id) || self.reservations.contains(&id) {
            return Err(TicketInsertError::Duplicate);
        }
        if self.values.len() + self.reservations.len() >= self.capacity {
            return Err(TicketInsertError::Busy);
        }
        self.values.insert(id, value);
        Ok(())
    }

    fn take(&mut self, id: &str) -> Option<T> {
        self.values.remove(id)
    }

    fn reserve(&mut self, id: &str) -> Option<T> {
        let value = self.values.remove(id)?;
        let inserted = self.reservations.insert(id.to_owned());
        debug_assert!(inserted, "a live ticket cannot already be reserved");
        Some(value)
    }

    fn release_reservation(&mut self, id: &str) -> bool {
        self.reservations.remove(id)
    }

    fn restore_reservation(&mut self, id: &str, value: T) -> bool {
        if !self.reservations.remove(id) || self.values.contains_key(id) {
            return false;
        }
        self.values.insert(id.to_owned(), value);
        true
    }
}

impl DiscoveryCredentialLeaseRegistry {
    fn new(capacity: usize) -> Self {
        Self {
            values: HashMap::new(),
            capacity,
        }
    }

    fn insert(
        &mut self,
        binding: DiscoveryCredentialLeaseBinding,
        credential: NativeCredential,
    ) -> CommandResult<()> {
        if !self.values.contains_key(&binding.session_id) && self.values.len() >= self.capacity {
            return Err(CommandError::busy());
        }
        self.values.insert(
            binding.session_id.clone(),
            DiscoveryCredentialLease {
                binding,
                credential,
            },
        );
        Ok(())
    }

    fn status(&mut self, binding: &DiscoveryCredentialLeaseBinding) -> CredentialStatus {
        let Some(entry) = self.values.get(&binding.session_id) else {
            return CredentialStatus::Missing;
        };
        if entry.binding == *binding {
            return CredentialStatus::Available;
        }
        // A stale secret must not remain available for a revised origin or
        // connection graph. Removal drops and zeroizes the native value.
        self.values.remove(&binding.session_id);
        CredentialStatus::Unreadable
    }

    fn credential_for_request(
        &mut self,
        binding: &DiscoveryCredentialLeaseBinding,
    ) -> CommandResult<Option<SecretCredential>> {
        match self.status(binding) {
            CredentialStatus::Missing => Ok(None),
            CredentialStatus::Unreadable => Err(CommandError::invalid_input()),
            CredentialStatus::Available => self
                .values
                .get(&binding.session_id)
                .map(|entry| SecretCredential::new(entry.credential.expose().to_owned()))
                .map(Some)
                .ok_or_else(CommandError::internal),
        }
    }

    fn take_for_commit(
        &mut self,
        session_id: &str,
        connection_id: &str,
        connection_binding_sha256: &str,
    ) -> CommandResult<Option<NativeCredential>> {
        let Some(entry) = self.values.get(session_id) else {
            return Ok(None);
        };
        if entry.binding.connection_id != connection_id
            || entry.binding.connection_binding_sha256 != connection_binding_sha256
        {
            self.values.remove(session_id);
            return Err(CommandError::invalid_input());
        }
        self.values
            .remove(session_id)
            .map(|entry| entry.credential)
            .map(Some)
            .ok_or_else(CommandError::internal)
    }

    fn matches_commit(
        &mut self,
        session_id: &str,
        connection_id: &str,
        connection_binding_sha256: &str,
    ) -> CommandResult<bool> {
        let Some(entry) = self.values.get(session_id) else {
            return Ok(false);
        };
        if entry.binding.connection_id != connection_id
            || entry.binding.connection_binding_sha256 != connection_binding_sha256
        {
            self.values.remove(session_id);
            return Err(CommandError::invalid_input());
        }
        Ok(true)
    }

    fn clear(&mut self, session_id: &str) {
        self.values.remove(session_id);
    }
}

impl ChatStreamRegistry {
    fn new(capacity: usize) -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
            capacity,
        }
    }

    fn register(self: &Arc<Self>, stream_id: &str) -> CommandResult<ChatStreamRegistration> {
        validate_stream_id(stream_id)?;
        let mut slots = self.slots.lock().map_err(|_| CommandError::internal())?;
        if slots.contains_key(stream_id) {
            return Err(CommandError::invalid_input());
        }
        if slots.len() >= self.capacity {
            return Err(CommandError::busy());
        }

        let marker = Arc::new(());
        let (dispose, dispose_receiver) = oneshot::channel();
        slots.insert(
            stream_id.to_owned(),
            ChatStreamSlot {
                marker: Arc::clone(&marker),
                dispose: Some(dispose),
            },
        );
        Ok(ChatStreamRegistration {
            stream_id: stream_id.to_owned(),
            marker,
            dispose: dispose_receiver,
            registry: Arc::clone(self),
        })
    }

    fn dispose(&self, stream_id: &str) -> CommandResult<bool> {
        validate_stream_id(stream_id)?;
        let mut slots = self.slots.lock().map_err(|_| CommandError::internal())?;
        let Some(slot) = slots.get_mut(stream_id) else {
            return Ok(false);
        };
        let Some(dispose) = slot.dispose.take() else {
            return Ok(false);
        };
        let _ = dispose.send(());
        Ok(true)
    }

    fn finish(&self, stream_id: &str, marker: &Arc<()>) {
        let Ok(mut slots) = self.slots.lock() else {
            return;
        };
        if slots
            .get(stream_id)
            .is_some_and(|slot| Arc::ptr_eq(&slot.marker, marker))
        {
            slots.remove(stream_id);
        }
    }
}

impl ChatStreamRegistration {
    pub(crate) async fn disposed(&mut self) {
        let _ = (&mut self.dispose).await;
    }
}

impl Drop for ChatStreamRegistration {
    fn drop(&mut self) {
        self.registry.finish(&self.stream_id, &self.marker);
    }
}

fn validate_stream_id(stream_id: &str) -> CommandResult<()> {
    if Uuid::parse_str(stream_id).is_ok_and(|value| value.to_string() == stream_id) {
        Ok(())
    } else {
        Err(CommandError::invalid_input())
    }
}

fn validate_delivery_id(delivery_id: &str) -> CommandResult<()> {
    if Uuid::parse_str(delivery_id).is_ok_and(|value| value.to_string() == delivery_id) {
        Ok(())
    } else {
        Err(CommandError::invalid_input())
    }
}

impl<T> TicketReservation<T> {
    fn new(ticket_id: String, value: T, store: Arc<Mutex<TicketStore<T>>>) -> Self {
        Self {
            ticket_id,
            value: Some(value),
            store,
        }
    }

    pub(crate) fn value(&self) -> &T {
        self.value
            .as_ref()
            .expect("a live reservation retains its ticket value")
    }

    pub(crate) fn complete(mut self) -> CommandResult<()> {
        let released = self
            .store
            .lock()
            .map_err(|_| CommandError::internal())?
            .release_reservation(&self.ticket_id);
        if !released {
            return Err(CommandError::internal());
        }
        drop(self.value.take());
        Ok(())
    }
}

impl<T> Drop for TicketReservation<T> {
    fn drop(&mut self) {
        let Some(value) = self.value.take() else {
            return;
        };
        let mut tickets = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = tickets.restore_reservation(&self.ticket_id, value);
    }
}

impl AppState {
    #[cfg(test)]
    pub fn new(data_root: PathBuf) -> Self {
        Self::new_inner(data_root, None)
    }

    pub fn new_with_app(data_root: PathBuf, app: AppHandle) -> Self {
        Self::new_inner(data_root, Some(app))
    }

    fn new_inner(data_root: PathBuf, app: Option<AppHandle>) -> Self {
        Self {
            data_root,
            app,
            shell: Mutex::new(None),
            startup: AsyncMutex::new(()),
            ready: AtomicBool::new(false),
            import_tickets: Arc::new(Mutex::new(TicketStore::new(MAXIMUM_IMPORT_TICKETS))),
            catalog_tickets: Mutex::new(TicketStore::new(MAXIMUM_CATALOG_TICKETS)),
            chat_streams: Arc::new(ChatStreamRegistry::new(MAXIMUM_CHAT_STREAMS)),
            memory_supervisor_shutdown: Mutex::new(None),
            memory_supervisor_status: Arc::new(Mutex::new(MemorySupervisorStatusDto {
                sequence: 0,
                phase: MemorySupervisorPhaseDto::NotStarted,
                recovered_interrupted_jobs: 0,
                completed_jobs: 0,
            })),
            interaction_supervisor_shutdown: Mutex::new(None),
            interaction_deliveries: Arc::new(Mutex::new(InteractionDeliveryRegistry::new())),
            lifecycle_supervisor_shutdown: Mutex::new(None),
            provider_credential_operation: Arc::new(AsyncRwLock::new(())),
            legacy_credential_admission: Arc::new(AsyncMutex::new(())),
            discovery_credential_leases: Mutex::new(DiscoveryCredentialLeaseRegistry::new(
                MAXIMUM_DISCOVERY_CREDENTIAL_LEASES,
            )),
        }
    }

    /// Serializes credential mutations against every in-flight provider read
    /// lease. Multiple provider dispatches may overlap, including primary
    /// generation planning that performs auxiliary provider work.
    pub(crate) async fn lock_provider_credential_operation(&self) -> AsyncRwLockWriteGuard<'_, ()> {
        self.provider_credential_operation.write().await
    }

    pub(crate) async fn lock_legacy_credential_admission(&self) -> AsyncMutexGuard<'_, ()> {
        self.legacy_credential_admission.lock().await
    }

    pub(crate) async fn lock_legacy_provider_credential_archive(
        &self,
    ) -> (AsyncMutexGuard<'_, ()>, AsyncRwLockWriteGuard<'_, ()>) {
        let legacy = self.lock_legacy_credential_admission().await;
        let provider = self.lock_provider_credential_operation().await;
        (legacy, provider)
    }

    pub(crate) fn install_discovery_credential_lease(
        &self,
        binding: DiscoveryCredentialLeaseBinding,
        credential: NativeCredential,
    ) -> CommandResult<()> {
        self.discovery_credential_leases
            .lock()
            .map_err(|_| CommandError::internal())?
            .insert(binding, credential)
    }

    pub(crate) fn discovery_credential_lease_status(
        &self,
        binding: &DiscoveryCredentialLeaseBinding,
    ) -> CredentialStatus {
        self.discovery_credential_leases
            .lock()
            .map_or(CredentialStatus::Unreadable, |mut leases| {
                leases.status(binding)
            })
    }

    pub(crate) fn discovery_credential_for_request(
        &self,
        binding: &DiscoveryCredentialLeaseBinding,
    ) -> CommandResult<Option<SecretCredential>> {
        self.discovery_credential_leases
            .lock()
            .map_err(|_| CommandError::internal())?
            .credential_for_request(binding)
    }

    pub(crate) fn take_discovery_credential_lease_for_commit(
        &self,
        session_id: &str,
        connection_id: &str,
        connection_binding_sha256: &str,
    ) -> CommandResult<Option<NativeCredential>> {
        self.discovery_credential_leases
            .lock()
            .map_err(|_| CommandError::internal())?
            .take_for_commit(session_id, connection_id, connection_binding_sha256)
    }

    pub(crate) fn discovery_credential_lease_matches_commit(
        &self,
        session_id: &str,
        connection_id: &str,
        connection_binding_sha256: &str,
    ) -> CommandResult<bool> {
        self.discovery_credential_leases
            .lock()
            .map_err(|_| CommandError::internal())?
            .matches_commit(session_id, connection_id, connection_binding_sha256)
    }

    pub(crate) fn clear_discovery_credential_lease(&self, session_id: &str) {
        if let Ok(mut leases) = self.discovery_credential_leases.lock() {
            leases.clear(session_id);
        }
    }

    pub(crate) async fn lease_provider_credential_operation(&self) -> OwnedRwLockReadGuard<()> {
        Arc::clone(&self.provider_credential_operation)
            .read_owned()
            .await
    }

    pub(crate) async fn lease_legacy_credential_admission(&self) -> OwnedMutexGuard<()> {
        Arc::clone(&self.legacy_credential_admission)
            .lock_owned()
            .await
    }

    /// Opens Core on demand, reconciles native discovery state, and publishes
    /// the shell only after startup recovery has completed.
    pub async fn bootstrap(&self, app: &AppHandle) -> CommandResult<BootstrapDto> {
        self.bootstrap_with_recovery(|shell| async move {
            crate::credential_operations::recover_provider_credential_operations(app, &shell)
                .await?;
            crate::provider_commands::recover_provider_discovery_with_shell(app, &shell)
                .await
                .map(|_| ())
        })
        .await
    }

    async fn bootstrap_with_recovery<F, Fut>(&self, recover: F) -> CommandResult<BootstrapDto>
    where
        F: FnOnce(ShellApi) -> Fut,
        Fut: Future<Output = CommandResult<()>>,
    {
        let _startup = self.startup.lock().await;
        if let Some(shell) = self
            .shell
            .lock()
            .map_err(|_| CommandError::core_unavailable())?
            .as_ref()
            .cloned()
        {
            let bootstrap = shell.bootstrap().map_err(CommandError::from)?;
            self.start_memory_supervisor(shell.clone())?;
            self.start_interaction_supervisor(shell.clone())?;
            self.start_lifecycle_supervisor(shell)?;
            self.emit_memory_supervisor_status()?;
            self.ready.store(true, Ordering::Release);
            return Ok(bootstrap);
        }

        let shell = ShellApi::open_data_root_for_native_discovery_recovery(&self.data_root)
            .map_err(CommandError::from)?;
        let recovered = shell
            .recover_running_memory_jobs()
            .map_err(CommandError::from)?;
        let provider_operation = self.lock_provider_credential_operation().await;
        recover(shell.clone()).await?;
        drop(provider_operation);
        let bootstrap = shell.bootstrap().map_err(CommandError::from)?;
        self.update_memory_supervisor_status(|status| {
            status.sequence = status.sequence.saturating_add(1);
            status.phase = MemorySupervisorPhaseDto::Recovered;
            status.recovered_interrupted_jobs = u32::try_from(recovered).unwrap_or(u32::MAX);
        })?;
        {
            let mut slot = self
                .shell
                .lock()
                .map_err(|_| CommandError::core_unavailable())?;
            if slot.is_some() {
                return Err(CommandError::internal());
            }
            *slot = Some(shell.clone());
        }
        self.start_memory_supervisor(shell.clone())?;
        self.start_interaction_supervisor(shell.clone())?;
        self.start_lifecycle_supervisor(shell)?;
        self.emit_memory_supervisor_status()?;
        self.ready.store(true, Ordering::Release);
        Ok(bootstrap)
    }

    fn start_memory_supervisor(&self, shell: ShellApi) -> CommandResult<()> {
        let Some(app) = self.app.clone() else {
            return Ok(());
        };
        let mut shutdown_slot = self
            .memory_supervisor_shutdown
            .lock()
            .map_err(|_| CommandError::internal())?;
        if shutdown_slot.is_some() {
            return Ok(());
        }
        let (shutdown, mut shutdown_receiver) = watch::channel(false);
        *shutdown_slot = Some(shutdown);
        drop(shutdown_slot);

        self.update_memory_supervisor_status(|status| {
            status.sequence = status.sequence.saturating_add(1);
            status.phase = MemorySupervisorPhaseDto::Running;
        })?;
        let status = Arc::clone(&self.memory_supervisor_status);
        let event_app = app.clone();
        let credential_reader = PlatformTaskCredentialReader {
            app,
            shell: shell.clone(),
            inherited_dispatch_lease: None,
        };
        tauri::async_runtime::spawn(async move {
            loop {
                if *shutdown_receiver.borrow() {
                    break;
                }
                match shell
                    .execute_next_memory_job(&credential_reader, shutdown_receiver.clone())
                    .await
                {
                    Ok(true) => {
                        update_memory_status_and_emit(&status, &event_app, |current| {
                            current.sequence = current.sequence.saturating_add(1);
                            current.completed_jobs = current.completed_jobs.saturating_add(1);
                        });
                        continue;
                    }
                    Ok(false) => {}
                    Err(_) => {
                        update_memory_status_and_emit(&status, &event_app, |current| {
                            current.sequence = current.sequence.saturating_add(1);
                            current.phase = MemorySupervisorPhaseDto::Failed;
                        });
                        break;
                    }
                }
                tokio::select! {
                    result = shutdown_receiver.changed() => {
                        if result.is_err() || *shutdown_receiver.borrow() {
                            break;
                        }
                    }
                    () = tokio::time::sleep(MEMORY_SUPERVISOR_IDLE_POLL) => {}
                }
            }
        });
        Ok(())
    }

    fn start_interaction_supervisor(&self, shell: ShellApi) -> CommandResult<()> {
        let Some(app) = self.app.clone() else {
            return Ok(());
        };
        let mut shutdown_slot = self
            .interaction_supervisor_shutdown
            .lock()
            .map_err(|_| CommandError::internal())?;
        if shutdown_slot.is_some() {
            return Ok(());
        }
        let (shutdown, mut shutdown_receiver) = watch::channel(false);
        *shutdown_slot = Some(shutdown);
        drop(shutdown_slot);

        let deliveries = Arc::clone(&self.interaction_deliveries);
        tauri::async_runtime::spawn(async move {
            loop {
                if *shutdown_receiver.borrow() {
                    break;
                }
                if let Ok(claims) = shell.claim_interaction_effects() {
                    for claim in claims {
                        let event = match deliveries.lock() {
                            Ok(mut registry) => registry.register(claim),
                            Err(_) => break,
                        };
                        let event = match event {
                            Ok(event) => event,
                            Err(claim) => {
                                let _ = shell.retry_interaction_effect(
                                    &claim.event_id,
                                    claim.sequence,
                                    claim.delivery_attempts,
                                );
                                continue;
                            }
                        };
                        if app.emit(INTERACTION_EFFECT_EVENT, event.clone()).is_err() {
                            let claim = deliveries
                                .lock()
                                .ok()
                                .and_then(|mut registry| registry.remove(&event.delivery_id));
                            if let Some(claim) = claim {
                                let _ = shell.retry_interaction_effect(
                                    &claim.event_id,
                                    claim.sequence,
                                    claim.delivery_attempts,
                                );
                            }
                        }
                    }
                }
                tokio::select! {
                    result = shutdown_receiver.changed() => {
                        if result.is_err() || *shutdown_receiver.borrow() {
                            break;
                        }
                    }
                    () = tokio::time::sleep(INTERACTION_SUPERVISOR_IDLE_POLL) => {}
                }
            }
        });
        Ok(())
    }

    fn start_lifecycle_supervisor(&self, shell: ShellApi) -> CommandResult<()> {
        // A headless AppState is used by lifecycle/ownership tests and has no
        // application runtime to own a long-lived supervisor task. Match the
        // other supervisors: do not let a detached task retain Core after the
        // headless state itself is dropped.
        if self.app.is_none() {
            return Ok(());
        }
        let mut shutdown_slot = self
            .lifecycle_supervisor_shutdown
            .lock()
            .map_err(|_| CommandError::internal())?;
        if shutdown_slot.is_some() {
            return Ok(());
        }
        let (shutdown, mut shutdown_receiver) = watch::channel(false);
        *shutdown_slot = Some(shutdown);
        drop(shutdown_slot);

        tauri::async_runtime::spawn(async move {
            let _ = shell.recover_expired_core_lifecycle_occurrence_leases();
            loop {
                if *shutdown_receiver.borrow() {
                    break;
                }
                let processed = shell
                    .drain_core_lifecycle_occurrences(LIFECYCLE_SUPERVISOR_BATCH_SIZE)
                    .unwrap_or(false);
                if processed {
                    continue;
                }
                tokio::select! {
                    result = shutdown_receiver.changed() => {
                        if result.is_err() || *shutdown_receiver.borrow() {
                            break;
                        }
                    }
                    () = tokio::time::sleep(LIFECYCLE_SUPERVISOR_IDLE_POLL) => {}
                }
            }
        });
        Ok(())
    }

    pub fn list_interaction_effects(&self) -> CommandResult<Vec<InteractionEffectEventDto>> {
        self.interaction_deliveries
            .lock()
            .map(|registry| registry.pending_events())
            .map_err(|_| CommandError::internal())
    }

    pub fn acknowledge_interaction_effect(&self, delivery_id: &str) -> CommandResult<()> {
        validate_delivery_id(delivery_id)?;
        let shell = self.shell()?;
        let mut registry = self
            .interaction_deliveries
            .lock()
            .map_err(|_| CommandError::internal())?;
        if registry.is_completed(delivery_id) {
            return Ok(());
        }
        let claim = registry
            .pending
            .get(delivery_id)
            .cloned()
            .ok_or_else(CommandError::invalid_input)?;
        shell
            .acknowledge_interaction_effect(
                &claim.event_id,
                claim.sequence,
                claim.delivery_attempts,
            )
            .map_err(CommandError::from)?;
        registry.complete(delivery_id);
        Ok(())
    }

    pub fn retry_interaction_effect(&self, delivery_id: &str) -> CommandResult<()> {
        validate_delivery_id(delivery_id)?;
        let shell = self.shell()?;
        let mut registry = self
            .interaction_deliveries
            .lock()
            .map_err(|_| CommandError::internal())?;
        if registry.is_completed(delivery_id) {
            return Ok(());
        }
        let claim = registry
            .pending
            .get(delivery_id)
            .cloned()
            .ok_or_else(CommandError::invalid_input)?;
        shell
            .retry_interaction_effect(&claim.event_id, claim.sequence, claim.delivery_attempts)
            .map_err(CommandError::from)?;
        registry.remove(delivery_id);
        Ok(())
    }

    pub fn memory_supervisor_status(&self) -> CommandResult<MemorySupervisorStatusDto> {
        self.memory_supervisor_status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| CommandError::internal())
    }

    fn update_memory_supervisor_status(
        &self,
        update: impl FnOnce(&mut MemorySupervisorStatusDto),
    ) -> CommandResult<()> {
        let mut status = self
            .memory_supervisor_status
            .lock()
            .map_err(|_| CommandError::internal())?;
        update(&mut status);
        Ok(())
    }

    fn emit_memory_supervisor_status(&self) -> CommandResult<()> {
        let Some(app) = &self.app else {
            return Ok(());
        };
        app.emit(
            MEMORY_SUPERVISOR_STATUS_EVENT,
            self.memory_supervisor_status()?,
        )
        .map_err(|_| CommandError::internal())
    }

    pub fn shell(&self) -> CommandResult<ShellApi> {
        self.ensure_ready()?;
        self.shell
            .lock()
            .map_err(|_| CommandError::core_unavailable())?
            .as_ref()
            .cloned()
            .ok_or_else(CommandError::core_unavailable)
    }

    pub(crate) fn ensure_ready(&self) -> CommandResult<()> {
        if self.ready.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(CommandError::core_unavailable())
        }
    }

    pub(crate) fn register_chat_stream(
        &self,
        stream_id: &str,
    ) -> CommandResult<ChatStreamRegistration> {
        self.chat_streams.register(stream_id)
    }

    pub(crate) fn preflight_generation_subscription(
        &self,
        request: &SubscribeGenerationRequest,
    ) -> CommandResult<ChatEventStream> {
        self.shell()?
            .subscribe_generation(
                &request.generation_id,
                &request.conversation_id,
                &request.branch_id,
                request.sequence_baseline,
            )
            .map_err(|_| CommandError::generation_reattachment_unavailable())
    }

    pub(crate) fn admit_generation_subscription(
        &self,
        request: &SubscribeGenerationRequest,
        stream_id: &str,
    ) -> CommandResult<(ChatEventStream, ChatStreamRegistration)> {
        let stream = self.preflight_generation_subscription(request)?;
        let registration = self.register_chat_stream(stream_id)?;
        Ok((stream, registration))
    }

    pub(crate) fn dispose_chat_stream(&self, stream_id: &str) -> CommandResult<bool> {
        self.chat_streams.dispose(stream_id)
    }

    pub fn insert_import_ticket(
        &self,
        ticket_id: String,
        staged: StagedImport,
    ) -> CommandResult<()> {
        let mut tickets = self.tickets()?;
        insert_ticket(&mut tickets, ticket_id, staged)
    }

    pub fn take_import_ticket(&self, ticket_id: &str) -> CommandResult<StagedImport> {
        self.tickets()?
            .take(ticket_id)
            .ok_or_else(CommandError::invalid_input)
    }

    pub(crate) fn reserve_import_ticket(
        &self,
        ticket_id: &str,
    ) -> CommandResult<TicketReservation<StagedImport>> {
        let value = self
            .tickets()?
            .reserve(ticket_id)
            .ok_or_else(CommandError::invalid_input)?;
        Ok(TicketReservation::new(
            ticket_id.to_owned(),
            value,
            Arc::clone(&self.import_tickets),
        ))
    }

    fn tickets(&self) -> CommandResult<MutexGuard<'_, TicketStore<StagedImport>>> {
        self.import_tickets
            .lock()
            .map_err(|_| CommandError::internal())
    }

    pub fn insert_catalog_ticket(
        &self,
        ticket_id: String,
        ticket: CatalogImportTicket,
    ) -> CommandResult<()> {
        let mut tickets = self.catalog_tickets()?;
        insert_ticket(&mut tickets, ticket_id, ticket)
    }

    pub fn take_catalog_ticket(&self, ticket_id: &str) -> CommandResult<CatalogImportTicket> {
        self.catalog_tickets()?
            .take(ticket_id)
            .ok_or_else(CommandError::invalid_input)
    }

    pub fn discard_catalog_ticket(&self, ticket_id: &str) -> CommandResult<()> {
        self.take_catalog_ticket(ticket_id).map(drop)
    }

    /// Preserve the exact verified envelope and plan when Core rejects an
    /// activation so the frontend can explicitly retry or discard it.
    pub fn activate_catalog_ticket(
        &self,
        shell: &ShellApi,
        ticket_id: &str,
    ) -> CommandResult<ProviderCatalogImportResultDto> {
        let mut tickets = self.catalog_tickets()?;
        let ticket = tickets
            .take(ticket_id)
            .ok_or_else(CommandError::invalid_input)?;
        match shell.activate_signed_provider_catalog_import(ticket.plan.clone(), &ticket.envelope) {
            Ok(result) => Ok(result),
            Err(error) => {
                // The slot was removed while this same mutex was held, so
                // reinsertion cannot race the explicit capacity bound.
                tickets
                    .insert(ticket_id.to_owned(), ticket)
                    .expect("catalog retry slot remains reserved");
                Err(error.into())
            }
        }
    }

    fn catalog_tickets(&self) -> CommandResult<MutexGuard<'_, TicketStore<CatalogImportTicket>>> {
        self.catalog_tickets
            .lock()
            .map_err(|_| CommandError::internal())
    }
}

fn update_memory_status_and_emit(
    status: &Mutex<MemorySupervisorStatusDto>,
    app: &AppHandle,
    update: impl FnOnce(&mut MemorySupervisorStatusDto),
) {
    let Ok(mut status) = status.lock() else {
        return;
    };
    update(&mut status);
    let snapshot = status.clone();
    drop(status);
    let _ = app.emit(MEMORY_SUPERVISOR_STATUS_EVENT, snapshot);
}

fn insert_ticket<T>(store: &mut TicketStore<T>, id: String, value: T) -> CommandResult<()> {
    store.insert(id, value).map_err(|error| match error {
        TicketInsertError::Duplicate => CommandError::invalid_input(),
        TicketInsertError::Busy => CommandError::busy(),
    })
}

impl Drop for AppState {
    fn drop(&mut self) {
        if let Ok(shutdown) = self.memory_supervisor_shutdown.get_mut()
            && let Some(shutdown) = shutdown.take()
        {
            let _ = shutdown.send(true);
        }
        if let Ok(shutdown) = self.interaction_supervisor_shutdown.get_mut()
            && let Some(shutdown) = shutdown.take()
        {
            let _ = shutdown.send(true);
        }
        if let Ok(shutdown) = self.lifecycle_supervisor_shutdown.get_mut()
            && let Some(shutdown) = shutdown.take()
        {
            let _ = shutdown.send(true);
        }
        if let Ok(slot) = self.shell.get_mut() {
            let _ = slot.take();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use lorepia_shell_api::{
        ClaimedInteractionEffect, ConversationModeDto, CreateConversationInput,
        InteractionEffectDeliveryDto, InteractionEffectDto,
        InteractionEffectProjectionRejectionReasonDto, ShellApi,
    };
    use sha2::{Digest, Sha256};
    use tauri_plugin_lorepia_platform::{CredentialStatus, NativeCredential};
    use tempfile::tempdir;
    use uuid::Uuid;

    use crate::contract::{MemorySupervisorPhaseDto, SubscribeGenerationRequest};

    use super::AppState;
    use super::{
        ChatStreamRegistry, DiscoveryCredentialLeaseBinding, DiscoveryCredentialLeaseRegistry,
        InteractionDeliveryRegistry, MAXIMUM_CHAT_STREAMS, TicketInsertError, TicketReservation,
        TicketStore, provider_dispatch_lease, task_credential_read_with_lease,
    };

    const SHELL_API_WRITE_TITLE: &str = "Tauri AppState Shell API continuity evidence";
    const COMPATIBLE_RECOVERY_WRITE_TITLE: &str = "Source-compatible rollback continuity evidence";

    struct DropProbe(Arc<AtomicUsize>);

    fn interaction_claim(
        effect_id: &str,
        event_id: &str,
        delivery_attempts: u64,
    ) -> ClaimedInteractionEffect {
        ClaimedInteractionEffect {
            delivery: InteractionEffectDeliveryDto {
                effect_id: effect_id.to_owned(),
                conversation_id: "conversation-1".to_owned(),
                branch_id: "branch-1".to_owned(),
                resulting_state_revision: 1,
                event_created_at: "2026-08-03T00:00:00+00:00".to_owned(),
                effect: InteractionEffectDto::StateChanged,
            },
            event_id: event_id.to_owned(),
            sequence: 0,
            delivery_attempts,
        }
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn discovery_credential_binding(
        session_id: &str,
        connection_id: &str,
        marker: char,
    ) -> DiscoveryCredentialLeaseBinding {
        DiscoveryCredentialLeaseBinding {
            session_id: session_id.to_owned(),
            connection_id: connection_id.to_owned(),
            credential_origin_approval_id: format!("approval-{marker}"),
            credential_origin_grant_sha256: marker.to_string().repeat(64),
            connection_binding_sha256: marker.to_ascii_uppercase().to_string().repeat(64),
        }
    }

    #[test]
    fn discovery_credential_lease_is_bounded_and_exactly_bound() {
        let binding = discovery_credential_binding("session-a", "connection-a", 'a');
        let mut leases = DiscoveryCredentialLeaseRegistry::new(1);
        leases
            .insert(
                binding.clone(),
                NativeCredential::new("synthetic-precommit-secret".to_owned()),
            )
            .expect("insert exact precommit lease");
        assert_eq!(leases.status(&binding), CredentialStatus::Available);
        let borrowed = leases
            .credential_for_request(&binding)
            .expect("borrow exact request credential")
            .expect("credential exists");
        assert_eq!(format!("{borrowed:?}"), "SecretCredential([REDACTED])");

        let other_session = discovery_credential_binding("session-b", "connection-b", 'b');
        assert_eq!(
            leases
                .insert(
                    other_session,
                    NativeCredential::new("must-not-evict-live-secret".to_owned()),
                )
                .expect_err("a live lease consumes bounded capacity")
                .code,
            "busy"
        );

        let drifted = discovery_credential_binding("session-a", "connection-a", 'c');
        assert_eq!(leases.status(&drifted), CredentialStatus::Unreadable);
        assert_eq!(leases.status(&binding), CredentialStatus::Missing);
        assert!(
            leases
                .credential_for_request(&binding)
                .expect("missing lookup")
                .is_none()
        );
    }

    #[test]
    fn discovery_credential_lease_moves_once_into_commit_handoff_and_clears_on_restart() {
        let root = tempdir().expect("temporary root");
        let binding = discovery_credential_binding("session-a", "connection-a", 'a');
        let state = AppState::new(root.path().to_path_buf());
        state
            .install_discovery_credential_lease(
                binding.clone(),
                NativeCredential::new("synthetic-handoff-secret".to_owned()),
            )
            .expect("install process-local lease");
        assert_eq!(
            state.discovery_credential_lease_status(&binding),
            CredentialStatus::Available
        );
        let moved = state
            .take_discovery_credential_lease_for_commit(
                &binding.session_id,
                &binding.connection_id,
                &binding.connection_binding_sha256,
            )
            .expect("take exact lease")
            .expect("lease exists");
        assert_eq!(moved.expose(), "synthetic-handoff-secret");
        assert!(
            state
                .take_discovery_credential_lease_for_commit(
                    &binding.session_id,
                    &binding.connection_id,
                    &binding.connection_binding_sha256,
                )
                .expect("second take")
                .is_none(),
            "one runtime credential cannot be handed off twice"
        );

        state
            .install_discovery_credential_lease(
                binding.clone(),
                NativeCredential::new("synthetic-restart-secret".to_owned()),
            )
            .expect("reinstall runtime lease");
        let reopened = AppState::new(root.path().to_path_buf());
        assert_eq!(
            reopened.discovery_credential_lease_status(&binding),
            CredentialStatus::Missing,
            "process restart must never recover or adopt a precommit secret"
        );
        state.clear_discovery_credential_lease("session-a");
        assert_eq!(
            state.discovery_credential_lease_status(&binding),
            CredentialStatus::Missing
        );
    }

    #[tokio::test]
    async fn bootstrap_is_lazy_and_drop_releases_core_owner() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        assert!(state.shell().is_err());
        state
            .bootstrap_with_recovery(|_| async { Ok(()) })
            .await
            .expect("bootstrap");
        assert!(state.shell().is_ok());
        drop(state);

        ShellApi::open_data_root(root.path()).expect("owner released after state drop");
    }

    #[tokio::test]
    #[ignore = "requires an exact frozen schema-11 external root and runtime evidence path"]
    async fn tauri_app_state_shell_api_writes_active_generation_for_compatible_recovery() {
        let root = external_schema_eleven_root();
        let state_path = external_schema_eleven_state_path();
        let canonical = root.join("db/lorepia.sqlite3");
        let canonical_sha256 = file_sha256(&canonical);

        let state = AppState::new(root.clone());
        assert!(
            state.shell().is_err(),
            "the Shell API must remain unpublished before Tauri bootstrap"
        );
        let bootstrap = state
            .bootstrap_with_recovery(|_| async { Ok(()) })
            .await
            .expect("bootstrap Tauri AppState over the frozen root");
        assert!(bootstrap.health.schema_version > 11);
        let shell = state.shell().expect("bootstrap publishes the Shell API");
        let character = shell
            .list_characters()
            .expect("list frozen fixture characters through Shell API")
            .into_iter()
            .next()
            .expect("frozen fixture character");
        let written = shell
            .create_conversation(CreateConversationInput {
                character_id: character.id,
                title: SHELL_API_WRITE_TITLE.to_owned(),
                mode: ConversationModeDto::Chat,
                greeting: None,
            })
            .expect("persist write A through the production Shell API boundary");
        assert_eq!(
            shell
                .get_conversation(&written.id)
                .expect("read Shell API write A before AppState shutdown")
                .title,
            SHELL_API_WRITE_TITLE
        );
        let active_relative = active_database_relative_path(&root);
        drop(shell);
        drop(state);

        assert_eq!(file_sha256(&canonical), canonical_sha256);
        let runtime_state = serde_json::json!({
            "format_version": 1,
            "root": root,
            "canonical_database_sha256": canonical_sha256,
            "active_database_relative_path": active_relative,
            "post_cutover_conversation_id": written.id,
            "post_cutover_conversation_title": SHELL_API_WRITE_TITLE,
            "post_cutover_conversation_visible_in_canonical": false,
            "post_cutover_conversation_visible_in_active": true
        });
        fs::write(
            state_path,
            serde_json::to_vec_pretty(&runtime_state).expect("encode Shell API runtime evidence"),
        )
        .expect("write Shell API runtime evidence");
    }

    #[tokio::test]
    #[ignore = "requires the prebuilt source-compatible Shell API client runtime evidence"]
    async fn tauri_app_state_shell_api_reopens_compatible_recovery_writes() {
        let root = external_schema_eleven_root();
        let runtime_state = serde_json::from_slice::<serde_json::Value>(
            &fs::read(external_schema_eleven_state_path())
                .expect("read compatible recovery runtime evidence"),
        )
        .expect("parse compatible recovery runtime evidence");
        assert_eq!(runtime_state["format_version"].as_u64(), Some(1));
        assert_eq!(
            PathBuf::from(
                runtime_state["root"]
                    .as_str()
                    .expect("runtime evidence root")
            ),
            root
        );

        let state = AppState::new(root.clone());
        let bootstrap = state
            .bootstrap_with_recovery(|_| async { Ok(()) })
            .await
            .expect("reopen Tauri AppState after compatible recovery write B");
        assert!(bootstrap.health.schema_version > 11);
        let shell = state.shell().expect("bootstrap republishes the Shell API");
        for (id_field, expected_title) in [
            ("post_cutover_conversation_id", SHELL_API_WRITE_TITLE),
            (
                "compatible_rollback_conversation_id",
                COMPATIBLE_RECOVERY_WRITE_TITLE,
            ),
        ] {
            let conversation_id = runtime_state[id_field]
                .as_str()
                .unwrap_or_else(|| panic!("runtime evidence is missing {id_field}"));
            assert_eq!(
                shell
                    .get_conversation(conversation_id)
                    .unwrap_or_else(|error| panic!("Shell API cannot read {id_field}: {error}"))
                    .title,
                expected_title
            );
        }
        assert_eq!(
            file_sha256(&root.join("db/lorepia.sqlite3")),
            runtime_state["canonical_database_sha256"]
                .as_str()
                .expect("canonical database SHA-256")
        );
        let active_relative = PathBuf::from(
            runtime_state["active_database_relative_path"]
                .as_str()
                .expect("active database relative path"),
        );
        assert!(!active_relative.is_absolute());
        assert!(root.join(active_relative).is_file());
        drop(shell);
        drop(state);
    }

    fn external_schema_eleven_root() -> PathBuf {
        PathBuf::from(
            env::var_os("LOREPIA_SCHEMA11_RUNTIME_ROOT")
                .expect("LOREPIA_SCHEMA11_RUNTIME_ROOT is required"),
        )
    }

    fn external_schema_eleven_state_path() -> PathBuf {
        PathBuf::from(
            env::var_os("LOREPIA_SCHEMA11_RUNTIME_STATE")
                .expect("LOREPIA_SCHEMA11_RUNTIME_STATE is required"),
        )
    }

    fn active_database_relative_path(root: &Path) -> PathBuf {
        let mut selected: Option<(u64, PathBuf)> = None;
        for entry in
            fs::read_dir(root.join("db/schema-cutover")).expect("read database generations")
        {
            let entry = entry.expect("read database generation entry");
            if !entry.path().join("generation-committed.json").is_file() {
                continue;
            }
            let manifest = serde_json::from_slice::<serde_json::Value>(
                &fs::read(entry.path().join("generation-manifest.json"))
                    .expect("read database generation manifest"),
            )
            .expect("parse database generation manifest");
            let sequence = manifest["activation_sequence"]
                .as_u64()
                .expect("database generation activation sequence");
            let relative = PathBuf::from(
                manifest["active_database_relative_path"]
                    .as_str()
                    .expect("active database relative path"),
            );
            if selected
                .as_ref()
                .is_none_or(|(current, _)| sequence > *current)
            {
                selected = Some((sequence, relative));
            }
        }
        let (_, relative) = selected.expect("active committed database generation");
        assert!(!relative.is_absolute());
        assert!(root.join(&relative).is_file());
        relative
    }

    fn file_sha256(path: &Path) -> String {
        format!(
            "{:x}",
            Sha256::digest(fs::read(path).expect("read file for SHA-256"))
        )
    }

    #[tokio::test]
    async fn repeated_bootstrap_does_not_repeat_memory_startup_recovery() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let initial = state.memory_supervisor_status().expect("initial status");
        assert_eq!(initial.sequence, 0);
        assert_eq!(initial.phase, MemorySupervisorPhaseDto::NotStarted);

        state
            .bootstrap_with_recovery(|_| async { Ok(()) })
            .await
            .expect("first bootstrap");
        let recovered = state.memory_supervisor_status().expect("recovered status");
        assert_eq!(recovered.sequence, 1);
        assert_eq!(recovered.phase, MemorySupervisorPhaseDto::Recovered);

        state
            .bootstrap_with_recovery(|_| async { Ok(()) })
            .await
            .expect("second bootstrap");
        assert_eq!(
            state
                .memory_supervisor_status()
                .expect("stable startup status"),
            recovered,
            "reopening the UI must not rerun startup recovery or auto-retry jobs"
        );
    }

    #[tokio::test]
    async fn provider_dispatch_lease_and_command_lock_share_one_gate() {
        let root = tempdir().expect("temporary root");
        let state = Arc::new(AppState::new(root.path().to_path_buf()));
        let dispatch_lease = state.lease_provider_credential_operation().await;
        let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
        let (acquired_sender, mut acquired_receiver) = tokio::sync::oneshot::channel();
        let command_state = Arc::clone(&state);
        let command = tokio::spawn(async move {
            entered_sender.send(()).expect("signal command entry");
            let _operation = command_state.lock_provider_credential_operation().await;
            let _ = acquired_sender.send(());
        });

        entered_receiver.await.expect("credential command entered");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                &mut acquired_receiver,
            )
            .await
            .is_err(),
            "archive/delete command must wait while provider dispatch owns the lease"
        );
        drop(dispatch_lease);
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut acquired_receiver)
            .await
            .expect("credential command released")
            .expect("credential command acquired lock");
        command.await.expect("credential command lock task");
    }

    #[tokio::test]
    async fn primary_and_auxiliary_provider_leases_overlap_while_mutation_waits() {
        let root = tempdir().expect("temporary root");
        let state = Arc::new(AppState::new(root.path().to_path_buf()));
        let primary = state.lease_provider_credential_operation().await;

        let auxiliary = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            state.lease_provider_credential_operation(),
        )
        .await
        .expect("prompt-time auxiliary dispatch must not deadlock behind primary generation");

        let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
        let (acquired_sender, mut acquired_receiver) = tokio::sync::oneshot::channel();
        let mutation_state = Arc::clone(&state);
        let mutation = tokio::spawn(async move {
            entered_sender.send(()).expect("signal mutation entry");
            let _operation = mutation_state.lock_provider_credential_operation().await;
            let _ = acquired_sender.send(());
        });
        entered_receiver.await.expect("credential mutation entered");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                &mut acquired_receiver,
            )
            .await
            .is_err(),
            "replacement/removal must wait for every provider dispatch lease"
        );

        drop(primary);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                &mut acquired_receiver,
            )
            .await
            .is_err(),
            "one remaining auxiliary dispatch lease must still block mutation"
        );
        drop(auxiliary);
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut acquired_receiver)
            .await
            .expect("credential mutation released")
            .expect("credential mutation acquired write lease");
        mutation.await.expect("credential mutation task");
    }

    #[tokio::test]
    async fn queued_mutation_does_not_deadlock_inherited_generation_auxiliary_lease() {
        let root = tempdir().expect("temporary root");
        let state = Arc::new(AppState::new(root.path().to_path_buf()));
        let primary = lorepia_shell_api::TaskCredentialLease::new(
            state.lease_provider_credential_operation().await,
        );
        let (writer_entered_sender, writer_entered_receiver) = tokio::sync::oneshot::channel();
        let (writer_acquired_sender, mut writer_acquired_receiver) =
            tokio::sync::oneshot::channel();
        let writer_state = Arc::clone(&state);
        let writer = tokio::spawn(async move {
            writer_entered_sender.send(()).expect("signal writer entry");
            let _operation = writer_state.lock_provider_credential_operation().await;
            let _ = writer_acquired_sender.send(());
        });
        writer_entered_receiver
            .await
            .expect("mutation writer entered");

        let auxiliary = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            provider_dispatch_lease(&state, Some(&primary)),
        )
        .await
        .expect("same-generation auxiliary lease must not queue behind a mutation writer");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                provider_dispatch_lease(&state, None),
            )
            .await
            .is_err(),
            "an independent provider read must remain ordered behind the queued mutation"
        );
        drop(primary);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                &mut writer_acquired_receiver,
            )
            .await
            .is_err(),
            "the inherited auxiliary clone must keep mutation blocked"
        );
        drop(auxiliary);
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            &mut writer_acquired_receiver,
        )
        .await
        .expect("mutation writer acquired after inherited leases")
        .expect("mutation writer acquisition signal");
        tokio::time::timeout(std::time::Duration::from_secs(1), writer)
            .await
            .expect("mutation writer released")
            .expect("mutation writer task");
    }

    #[tokio::test]
    async fn missing_bound_read_keeps_command_lock_leased_until_carrier_drops() {
        let root = tempdir().expect("temporary root");
        let state = Arc::new(AppState::new(root.path().to_path_buf()));
        let dispatch_lease = lorepia_shell_api::TaskCredentialLease::new(
            state.lease_provider_credential_operation().await,
        );
        let credential_read = task_credential_read_with_lease(Ok(None), dispatch_lease);
        assert!(matches!(
            &credential_read,
            lorepia_shell_api::TaskCredentialRead::MissingWithLease(_)
        ));
        let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
        let (acquired_sender, mut acquired_receiver) = tokio::sync::oneshot::channel();
        let command_state = Arc::clone(&state);
        let command = tokio::spawn(async move {
            entered_sender.send(()).expect("signal command entry");
            let _operation = command_state.lock_provider_credential_operation().await;
            let _ = acquired_sender.send(());
        });

        entered_receiver.await.expect("credential command entered");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                &mut acquired_receiver,
            )
            .await
            .is_err(),
            "archive/delete command must wait while a missing credential carrier is live"
        );
        drop(credential_read);
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut acquired_receiver)
            .await
            .expect("credential command released")
            .expect("credential command acquired lock");
        command.await.expect("credential command lock task");
    }

    #[tokio::test]
    async fn legacy_admission_carrier_blocks_mutation_without_blocking_auxiliary_reads() {
        let root = tempdir().expect("temporary root");
        let state = Arc::new(AppState::new(root.path().to_path_buf()));
        let admission_lease = state.lease_legacy_credential_admission().await;
        let credential = lorepia_shell_api::GenerationCredential::legacy_with_admission_lease(
            None,
            admission_lease,
        );
        let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
        let (acquired_sender, mut acquired_receiver) = tokio::sync::oneshot::channel();
        let mutation_state = Arc::clone(&state);
        let mutation = tokio::spawn(async move {
            entered_sender.send(()).expect("signal mutation entry");
            let _operation = mutation_state.lock_legacy_credential_admission().await;
            let _ = acquired_sender.send(());
        });

        entered_receiver.await.expect("mutation waiter entered");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                &mut acquired_receiver,
            )
            .await
            .is_err(),
            "legacy mutation must wait until durable admission releases its carrier"
        );
        let auxiliary = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            state.lock_provider_credential_operation(),
        )
        .await
        .expect("prompt-time auxiliary credential reads use an independent mutex");
        drop(auxiliary);
        drop(credential);
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut acquired_receiver)
            .await
            .expect("mutation waiter released")
            .expect("mutation waiter acquired legacy lock");
        mutation.await.expect("legacy mutation lock task");
    }

    #[tokio::test]
    async fn legacy_alias_archive_acquires_legacy_before_global_provider_lock() {
        let root = tempdir().expect("temporary root");
        let state = Arc::new(AppState::new(root.path().to_path_buf()));
        let legacy = state.lease_legacy_credential_admission().await;
        let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
        let (acquired_sender, acquired_receiver) = tokio::sync::oneshot::channel();
        let archive_state = Arc::clone(&state);
        let archive = tokio::spawn(async move {
            entered_sender.send(()).expect("signal archive entry");
            let _guards = archive_state
                .lock_legacy_provider_credential_archive()
                .await;
            let _ = acquired_sender.send(());
        });
        entered_receiver.await.expect("archive waiter entered");

        let global = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            state.lock_provider_credential_operation(),
        )
        .await
        .expect("archive waiting on legacy must not preempt the global provider lock");
        drop(global);
        drop(legacy);
        tokio::time::timeout(std::time::Duration::from_secs(1), acquired_receiver)
            .await
            .expect("archive lock acquisition completed")
            .expect("archive acquired legacy then global locks");
        archive.await.expect("archive lock task");
    }

    #[tokio::test]
    async fn bootstrap_keeps_shell_and_supervisors_gated_until_recovery_finishes() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
        let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
        let second_recovery_calls = Arc::new(AtomicUsize::new(0));

        let first = state.bootstrap_with_recovery(|_| async move {
            entered_sender.send(()).expect("observe recovery start");
            release_receiver.await.expect("release startup recovery");
            Ok(())
        });
        let second = async {
            entered_receiver.await.expect("recovery started");
            assert!(state.shell().is_err());
            assert!(!state.ready.load(Ordering::Acquire));
            assert_eq!(
                state
                    .memory_supervisor_status()
                    .expect("startup-gated status")
                    .phase,
                MemorySupervisorPhaseDto::NotStarted
            );
            assert!(
                state
                    .memory_supervisor_shutdown
                    .lock()
                    .expect("memory supervisor lock")
                    .is_none()
            );
            assert!(
                state
                    .interaction_supervisor_shutdown
                    .lock()
                    .expect("interaction supervisor lock")
                    .is_none()
            );
            assert!(
                state
                    .lifecycle_supervisor_shutdown
                    .lock()
                    .expect("lifecycle supervisor lock")
                    .is_none()
            );
            release_sender.send(()).expect("release recovery");
            let calls = Arc::clone(&second_recovery_calls);
            state
                .bootstrap_with_recovery(move |_| async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
        };

        let (first, second) = tokio::join!(first, second);
        first.expect("first bootstrap");
        second.expect("concurrent bootstrap");
        assert!(state.shell().is_ok());
        assert!(state.ready.load(Ordering::Acquire));
        assert_eq!(second_recovery_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn interaction_redelivery_keeps_effect_identity_but_rotates_opaque_delivery_id() {
        let mut registry = InteractionDeliveryRegistry::new();
        let first = registry
            .register(interaction_claim("effect-1", "event-1", 1))
            .expect("first delivery");
        let second = registry
            .register(interaction_claim("effect-1", "event-1", 2))
            .expect("redelivery");

        assert_eq!(first.effect_id, second.effect_id);
        assert_ne!(first.delivery_id, second.delivery_id);
        assert!(!registry.pending.contains_key(&first.delivery_id));
        assert!(registry.pending.contains_key(&second.delivery_id));
        registry.complete(&second.delivery_id);
        registry.complete(&second.delivery_id);
        assert!(registry.is_completed(&second.delivery_id));
        assert!(registry.pending_events().is_empty());
    }

    #[test]
    fn rejected_interaction_projection_remains_content_free_and_acknowledgeable() {
        let mut registry = InteractionDeliveryRegistry::new();
        let mut claim = interaction_claim("effect-rejected", "event-rejected", 1);
        claim.delivery.effect = InteractionEffectDto::ProjectionRejected {
            reason: InteractionEffectProjectionRejectionReasonDto::InvalidStoredEffect,
        };

        let event = registry
            .register(claim)
            .expect("register rejected projection");
        assert_eq!(
            serde_json::to_value(&event.effect).expect("serialize rejected projection"),
            serde_json::json!({
                "kind": "projection_rejected",
                "reason": "invalid_stored_effect"
            })
        );
        assert_eq!(registry.pending_events(), vec![event.clone()]);

        registry.complete(&event.delivery_id);
        assert!(registry.is_completed(&event.delivery_id));
        assert!(registry.pending_events().is_empty());
    }

    #[test]
    fn ticket_store_is_bounded_and_never_replaces_a_live_ticket() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut tickets = TicketStore::new(1);
        tickets
            .insert("one".to_owned(), DropProbe(Arc::clone(&dropped)))
            .expect("first");
        assert_eq!(
            tickets.insert("one".to_owned(), DropProbe(Arc::clone(&dropped))),
            Err(TicketInsertError::Duplicate)
        );
        assert_eq!(
            tickets.insert("two".to_owned(), DropProbe(Arc::clone(&dropped))),
            Err(TicketInsertError::Busy)
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 2);

        let consumed = tickets.take("one").expect("consume");
        assert_eq!(dropped.load(Ordering::SeqCst), 2);
        drop(consumed);
        assert_eq!(dropped.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn dropping_ticket_store_discards_every_remaining_value() {
        let dropped = Arc::new(AtomicUsize::new(0));
        {
            let mut tickets = TicketStore::new(2);
            tickets
                .insert("one".to_owned(), DropProbe(Arc::clone(&dropped)))
                .expect("one");
            tickets
                .insert("two".to_owned(), DropProbe(Arc::clone(&dropped)))
                .expect("two");
        }
        assert_eq!(dropped.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn reservation_keeps_a_full_store_full_and_drop_restores_the_same_ticket() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let tickets = Arc::new(std::sync::Mutex::new(TicketStore::new(1)));
        tickets
            .lock()
            .expect("tickets")
            .insert("one".to_owned(), DropProbe(Arc::clone(&dropped)))
            .expect("first");
        let value = tickets
            .lock()
            .expect("tickets")
            .reserve("one")
            .expect("reserve");
        let reservation = TicketReservation::new("one".to_owned(), value, Arc::clone(&tickets));

        let concurrent_store = Arc::clone(&tickets);
        let concurrent_dropped = Arc::clone(&dropped);
        let insertion = std::thread::spawn(move || {
            concurrent_store
                .lock()
                .expect("tickets")
                .insert("two".to_owned(), DropProbe(Arc::clone(&concurrent_dropped)))
        })
        .join()
        .expect("insertion worker");
        assert_eq!(insertion, Err(TicketInsertError::Busy));
        assert_eq!(dropped.load(Ordering::SeqCst), 1);

        drop(reservation);
        let mut tickets = tickets.lock().expect("tickets");
        assert!(tickets.reservations.is_empty());
        assert!(tickets.values.contains_key("one"));
        assert_eq!(
            tickets.insert("one".to_owned(), DropProbe(Arc::clone(&dropped))),
            Err(TicketInsertError::Duplicate)
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn completing_a_reservation_releases_capacity() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let tickets = Arc::new(std::sync::Mutex::new(TicketStore::new(1)));
        tickets
            .lock()
            .expect("tickets")
            .insert("one".to_owned(), DropProbe(Arc::clone(&dropped)))
            .expect("first");
        let value = tickets
            .lock()
            .expect("tickets")
            .reserve("one")
            .expect("reserve");
        TicketReservation::new("one".to_owned(), value, Arc::clone(&tickets))
            .complete()
            .expect("complete");

        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        tickets
            .lock()
            .expect("tickets")
            .insert("two".to_owned(), DropProbe(Arc::clone(&dropped)))
            .expect("released capacity");
    }

    #[tokio::test]
    async fn chat_stream_registry_is_bounded_and_disposal_targets_one_registration() {
        const FIRST_ID: &str = "00000000-0000-4000-8000-000000000001";
        const SECOND_ID: &str = "00000000-0000-4000-8000-000000000002";

        let registry = Arc::new(ChatStreamRegistry::new(1));
        let mut first = registry.register(FIRST_ID).expect("first registration");
        assert_eq!(
            registry
                .register(FIRST_ID)
                .err()
                .expect("duplicate identifier")
                .code,
            "invalid_input"
        );
        assert_eq!(
            registry
                .register(SECOND_ID)
                .err()
                .expect("bounded registry")
                .code,
            "busy"
        );

        assert!(registry.dispose(FIRST_ID).expect("dispose first"));
        assert_eq!(
            registry
                .register(FIRST_ID)
                .err()
                .expect("disposing registration still owns its bounded slot")
                .code,
            "invalid_input"
        );
        first.disposed().await;
        drop(first);

        let second_lifetime = registry
            .register(FIRST_ID)
            .expect("identifier may be reused after forwarder exit");
        assert_eq!(
            registry
                .register(FIRST_ID)
                .err()
                .expect("old cleanup must not remove a reused identifier")
                .code,
            "invalid_input"
        );
        assert!(registry.dispose(FIRST_ID).expect("dispose second lifetime"));
        assert!(!registry.dispose(FIRST_ID).expect("idempotent disposal"));
        drop(second_lifetime);
        assert!(!registry.dispose(FIRST_ID).expect("idempotent disposal"));
    }

    #[test]
    fn chat_stream_registry_rejects_noncanonical_identifiers() {
        let registry = Arc::new(ChatStreamRegistry::new(1));
        assert_eq!(
            registry
                .register("not-an-opaque-stream-id")
                .err()
                .expect("invalid stream identifier")
                .code,
            "invalid_input"
        );
    }

    #[tokio::test]
    async fn generation_subscription_admission_preflights_before_consuming_stream_capacity() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        state
            .bootstrap_with_recovery(|_| async { Ok(()) })
            .await
            .expect("bootstrap");
        let request = SubscribeGenerationRequest {
            generation_id: Uuid::from_u128(10_001).to_string(),
            conversation_id: Uuid::from_u128(10_002).to_string(),
            branch_id: Uuid::from_u128(10_003).to_string(),
            sequence_baseline: 0,
        };

        let error = state
            .admit_generation_subscription(&request, "not-an-opaque-stream-id")
            .err()
            .expect("Core preflight must run before stream identifier admission");
        assert_eq!(error.code, "generation_reattachment_unavailable");

        for index in 0..(MAXIMUM_CHAT_STREAMS * 2) {
            let error = state
                .admit_generation_subscription(
                    &request,
                    &Uuid::from_u128(index as u128 + 1).to_string(),
                )
                .err()
                .expect("unknown generation cannot be reattached");
            assert_eq!(error.code, "generation_reattachment_unavailable");
        }

        let registrations = (0..MAXIMUM_CHAT_STREAMS)
            .map(|index| {
                state
                    .register_chat_stream(&Uuid::from_u128(index as u128 + 1).to_string())
                    .expect("rejected reattachments leave capacity for initial streams")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            state
                .register_chat_stream(
                    &Uuid::from_u128(MAXIMUM_CHAT_STREAMS as u128 + 1).to_string(),
                )
                .err()
                .expect("the registry retains its original independent bound")
                .code,
            "busy"
        );
        drop(registrations);
    }
}
