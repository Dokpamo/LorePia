use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use chrono::Utc;
use lorepia_domain::{
    CoreError, CoreErrorCode, CoreResult, ModelSyncDiff, ModelSyncEvent, ModelSyncFailure,
    ModelSyncJob, ModelSyncJobId, ModelSyncReview, ModelSyncSourceProvenance, ModelSyncState,
    ProviderConnection, ProviderConnectionId, ProviderTemplate,
};
use lorepia_providers::{AdapterRegistry, ModelListRequest, ModelListing};
use lorepia_storage::{ProviderCredentialAccessAuthority, Storage};
use tokio::sync::watch;

use super::{
    ConnectionBoundCredential, Core, ModelRecordSource,
    ensure_model_list_does_not_reflect_credential, initial_generation_preset,
    model_record_source_name, provider_api_capability_observations, reconcile_input_routes,
    record_model_refresh_failure, template_accepts_empty_preset, validate_provider_template,
};

const MODEL_SYNC_TEARDOWN_WAIT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct ModelSyncRegistry {
    active: Mutex<HashMap<ModelSyncJobId, watch::Sender<bool>>>,
    drained: Condvar,
    #[cfg(test)]
    model_listing_overrides: Mutex<HashMap<ProviderConnectionId, Arc<dyn ModelListing>>>,
}

fn build_model_listing(
    #[cfg(test)] registry: &ModelSyncRegistry,
    template: &ProviderTemplate,
    connection: &ProviderConnection,
) -> CoreResult<Arc<dyn ModelListing>> {
    #[cfg(test)]
    if let Some(listing) = registry
        .model_listing_overrides
        .lock()
        .map_err(|_| CoreError::internal("model listing override lock was poisoned"))?
        .get(&connection.id)
    {
        return Ok(Arc::clone(listing));
    }
    AdapterRegistry::new().build_model_listing(template, connection)
}

impl ModelSyncRegistry {
    #[cfg(test)]
    fn install_model_listing_override(
        &self,
        connection_id: ProviderConnectionId,
        listing: Arc<dyn ModelListing>,
    ) -> CoreResult<()> {
        let replaced = self
            .model_listing_overrides
            .lock()
            .map_err(|_| CoreError::internal("model listing override lock was poisoned"))?
            .insert(connection_id, listing);
        if replaced.is_some() {
            return Err(CoreError::internal(
                "model listing override was installed more than once",
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    fn cancellation_requested(&self, id: &ModelSyncJobId) -> CoreResult<bool> {
        let active = self
            .active
            .lock()
            .map_err(|_| CoreError::internal("model synchronization registry lock was poisoned"))?;
        Ok(active.get(id).is_some_and(|sender| *sender.borrow()))
    }

    fn register(&self, id: ModelSyncJobId, sender: watch::Sender<bool>) -> CoreResult<()> {
        let replaced = self
            .active
            .lock()
            .map_err(|_| CoreError::internal("model synchronization registry lock was poisoned"))?
            .insert(id, sender);
        if replaced.is_some() {
            return Err(CoreError::internal(
                "model synchronization was registered more than once",
            ));
        }
        Ok(())
    }

    fn cancel_and_wait_for_teardown(&self, id: &ModelSyncJobId) -> CoreResult<()> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| CoreError::internal("model synchronization registry lock was poisoned"))?;
        let Some(sender) = active.get(id).cloned() else {
            return Ok(());
        };
        let _ = sender.send(true);

        let deadline = Instant::now() + MODEL_SYNC_TEARDOWN_WAIT;
        while active.contains_key(id) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(CoreError::new(
                    CoreErrorCode::StorageUnavailable,
                    "model synchronization cancellation is still awaiting provider teardown",
                    true,
                ));
            }
            let (next, timeout) = self.drained.wait_timeout(active, remaining).map_err(|_| {
                CoreError::internal("model synchronization registry lock was poisoned")
            })?;
            active = next;
            if timeout.timed_out() && active.contains_key(id) {
                return Err(CoreError::new(
                    CoreErrorCode::StorageUnavailable,
                    "model synchronization cancellation is still awaiting provider teardown",
                    true,
                ));
            }
        }
        Ok(())
    }

    fn remove(&self, id: &ModelSyncJobId) {
        let removed = self
            .active
            .lock()
            .ok()
            .and_then(|mut active| active.remove(id))
            .is_some();
        if removed {
            self.drained.notify_all();
        }
    }

    pub(super) fn cancel_all(&self) {
        if let Ok(active) = self.active.lock() {
            for sender in active.values() {
                let _ = sender.send(true);
            }
        }
    }

    pub(super) fn len(&self) -> usize {
        self.active.lock().map_or(0, |active| active.len())
    }
}

struct ModelSyncTask {
    storage: Arc<Storage>,
    registry: Arc<ModelSyncRegistry>,
    job_id: ModelSyncJobId,
    connection_id: ProviderConnectionId,
    credential: Option<ConnectionBoundCredential>,
    cancel_receiver: watch::Receiver<bool>,
}

impl Core {
    /// Starts one durable, review-gated model synchronization.
    ///
    /// `credential` lives only in the spawned request task. It is never
    /// serialized into the job, review, outbox, route metadata, or error.
    pub fn start_provider_model_sync(
        &self,
        connection_id: &ProviderConnectionId,
        credential: Option<String>,
    ) -> CoreResult<ModelSyncJobId> {
        let connection = self.inner.storage.get_provider_connection(connection_id)?;
        let credential = ConnectionBoundCredential::new(connection.id.clone(), credential);
        self.start_provider_model_sync_inner(connection, credential, None, false)
    }

    /// Starts model synchronization with credential material bound to the
    /// exact durable access authority used by the native vault read.
    pub fn start_provider_model_sync_with_credential(
        &self,
        connection_id: &ProviderConnectionId,
        credential: ConnectionBoundCredential,
    ) -> CoreResult<ModelSyncJobId> {
        let connection = self.inner.storage.get_provider_connection(connection_id)?;
        credential.value_for_connection(&connection)?;
        let credential_access_authority = credential.access_authority().cloned();
        self.start_provider_model_sync_inner(
            connection,
            credential,
            credential_access_authority,
            true,
        )
    }

    fn start_provider_model_sync_inner(
        &self,
        connection: ProviderConnection,
        credential: ConnectionBoundCredential,
        credential_access_authority: Option<ProviderCredentialAccessAuthority>,
        require_exact_credential_access_authority: bool,
    ) -> CoreResult<ModelSyncJobId> {
        let template = self
            .inner
            .storage
            .get_provider_template(&connection.template_id, connection.template_version)?;
        validate_provider_template(&template)?;
        // Build before creating the job so unsupported templates do not leave
        // an inert active row behind.
        build_model_listing(
            #[cfg(test)]
            &self.inner.active_model_syncs,
            &template,
            &connection,
        )?;

        let job = if require_exact_credential_access_authority {
            self.inner
                .storage
                .create_model_sync_job_with_credential_access_authority(
                    &connection,
                    credential_access_authority.as_ref(),
                )?
        } else {
            self.inner.storage.create_model_sync_job(&connection)?
        };
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        if let Err(error) = self
            .inner
            .active_model_syncs
            .register(job.id.clone(), cancel_sender)
        {
            drop(credential);
            let _ = self.inner.storage.cancel_model_sync_job(&job.id);
            return Err(error);
        }
        let task = ModelSyncTask {
            storage: Arc::clone(&self.inner.storage),
            registry: Arc::clone(&self.inner.active_model_syncs),
            job_id: job.id.clone(),
            connection_id: connection.id,
            credential: Some(credential),
            cancel_receiver,
        };
        self.inner.runtime.spawn(run_model_sync(task));
        Ok(job.id)
    }

    pub fn get_provider_model_sync(&self, id: &ModelSyncJobId) -> CoreResult<ModelSyncJob> {
        self.inner.storage.get_model_sync_job(id)
    }

    pub fn list_provider_model_syncs(
        &self,
        connection_id: &ProviderConnectionId,
        limit: u32,
    ) -> CoreResult<Vec<ModelSyncJob>> {
        self.inner
            .storage
            .list_model_sync_jobs(connection_id, limit)
    }

    /// Approves exactly the currently stored canonical diff hash.
    pub fn approve_provider_model_sync(
        &self,
        id: &ModelSyncJobId,
        review_sha256: &str,
    ) -> CoreResult<ModelSyncJob> {
        let current = self.inner.storage.get_model_sync_job(id)?;
        if current.state == ModelSyncState::Completed {
            let review = current.review.as_ref().ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "completed model synchronization is missing its review",
                    false,
                )
            })?;
            review.verify().map_err(|message| {
                CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
            })?;
            if review.sha256 == review_sha256 {
                return Ok(current);
            }
            return Err(CoreError::invalid(
                "approved model synchronization hash does not match the completed review",
            ));
        }
        if current.state != ModelSyncState::DiffReadyAwaitingReview {
            return Err(CoreError::invalid(
                "model synchronization is not awaiting review",
            ));
        }
        let review = current.review.as_ref().ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "review-ready model synchronization is missing its review",
                false,
            )
        })?;
        review
            .verify()
            .map_err(|message| CoreError::new(CoreErrorCode::StorageCorrupted, message, false))?;
        if review.sha256 != review_sha256 {
            return Err(CoreError::invalid(
                "approved model synchronization hash does not match the current review",
            ));
        }

        let committing = self.inner.storage.mark_model_sync_job_committing(
            id,
            current.revision,
            review_sha256,
        )?;
        match self
            .inner
            .storage
            .commit_model_sync_job(id, committing.revision)
        {
            Ok(completed) => Ok(completed),
            Err(error) => {
                let failure = ModelSyncFailure::from_core_error(&error);
                let _ = self
                    .inner
                    .storage
                    .fail_model_sync_job(id, committing.revision, &failure);
                Err(error)
            }
        }
    }

    pub fn cancel_provider_model_sync(&self, id: &ModelSyncJobId) -> CoreResult<ModelSyncJob> {
        self.inner
            .active_model_syncs
            .cancel_and_wait_for_teardown(id)?;
        self.inner.storage.cancel_model_sync_job(id)
    }

    /// Polls durable progress events for one job with at-least-once delivery.
    ///
    /// Events remain available until the host acknowledges their exact
    /// `(job_id, sequence)` identity.
    pub fn poll_provider_model_sync_events(
        &self,
        id: &ModelSyncJobId,
        limit: u32,
    ) -> CoreResult<Vec<ModelSyncEvent>> {
        self.inner.storage.poll_model_sync_events_for_job(id, limit)
    }

    /// Acknowledges one event previously polled for this exact job.
    pub fn ack_provider_model_sync_event(
        &self,
        id: &ModelSyncJobId,
        sequence: u64,
    ) -> CoreResult<bool> {
        self.inner.storage.ack_model_sync_event(id, sequence)
    }
}

async fn run_model_sync(mut task: ModelSyncTask) {
    let outcome = run_model_sync_inner(&mut task).await;
    // The provider future has returned. Clear request-scoped secret material
    // before any terminal durable transition can allow native credential
    // removal.
    task.credential = None;

    // Failure payloads are intentionally created from the stable error code
    // only. Provider bodies/messages and credential text are never persisted.
    if let Err((revision, error)) = outcome {
        if error.code == CoreErrorCode::Cancelled {
            let _ = task.storage.cancel_model_sync_job(&task.job_id);
        } else {
            if let Ok(connection) = task.storage.get_provider_connection(&task.connection_id) {
                let _ = record_model_refresh_failure(&task.storage, &connection, &error);
            }
            let failure = ModelSyncFailure::from_core_error(&error);
            let _ = task
                .storage
                .fail_model_sync_job(&task.job_id, revision, &failure);
        }
    }
    // Notify cancellation waiters only after the task-owned terminal write.
    // The waiter can then replay cancellation idempotently without racing the
    // same state revision.
    task.registry.remove(&task.job_id);
}

#[allow(
    clippy::too_many_lines,
    reason = "the sync state machine keeps each durable checkpoint in one ordered workflow"
)]
async fn run_model_sync_inner(task: &mut ModelSyncTask) -> Result<(), (u64, CoreError)> {
    let created = task
        .storage
        .get_model_sync_job(&task.job_id)
        .map_err(|error| (1, error))?;
    let fetching = task
        .storage
        .transition_model_sync_job_to_fetching(&task.job_id, created.revision)
        .map_err(|error| (created.revision, error))?;
    if *task.cancel_receiver.borrow() {
        return Err((
            fetching.revision,
            CoreError::new(CoreErrorCode::Cancelled, "operation was cancelled", true),
        ));
    }
    let connection = task
        .storage
        .get_provider_connection(&task.connection_id)
        .map_err(|error| (fetching.revision, error))?;
    let template = task
        .storage
        .get_provider_template(&connection.template_id, connection.template_version)
        .map_err(|error| (fetching.revision, error))?;
    validate_provider_template(&template).map_err(|error| (fetching.revision, error))?;
    let listing = build_model_listing(
        #[cfg(test)]
        &task.registry,
        &template,
        &connection,
    )
    .map_err(|error| (fetching.revision, error))?;
    let listed = listing
        .list_models(ModelListRequest::new(
            task.credential
                .as_ref()
                .and_then(|credential| credential.value.as_deref()),
            task.cancel_receiver.clone(),
        ))
        .await
        .map_err(|error| (fetching.revision, error))?;
    ensure_model_list_does_not_reflect_credential(
        &listed,
        task.credential
            .as_ref()
            .and_then(|credential| credential.value.as_deref()),
    )
    .map_err(|error| (fetching.revision, error))?;
    if *task.cancel_receiver.borrow() {
        return Err((
            fetching.revision,
            CoreError::new(CoreErrorCode::Cancelled, "operation was cancelled", true),
        ));
    }

    let current_connection = task
        .storage
        .get_provider_connection(&task.connection_id)
        .map_err(|error| (fetching.revision, error))?;
    if current_connection != connection {
        return Err((
            fetching.revision,
            CoreError::invalid("provider connection changed while its model list was refreshing"),
        ));
    }
    let observed_at = Utc::now();
    let existing_routes = task
        .storage
        .list_model_routes(&task.connection_id)
        .map_err(|error| (fetching.revision, error))?;
    let (mut listed_routes, newly_seen_model_route_ids, missing_model_route_ids) =
        reconcile_input_routes(
            &task.connection_id,
            template.api_family,
            &existing_routes,
            &listed.models,
            observed_at,
        )
        .map_err(|error| (fetching.revision, error))?;
    for route in &mut listed_routes {
        route.last_reconciled_sync_job_id = Some(task.job_id.clone());
        route.metadata_sync_job_id = Some(task.job_id.clone());
    }
    let can_create_initial_preset =
        template_accepts_empty_preset(&template).map_err(|error| (fetching.revision, error))?;
    let mut initial_presets = Vec::new();
    let mut routes_requiring_preset_configuration = Vec::new();
    for route_id in &newly_seen_model_route_ids {
        if can_create_initial_preset {
            initial_presets.push(initial_generation_preset(route_id, &template, observed_at));
        } else {
            routes_requiring_preset_configuration.push(route_id.clone());
        }
    }
    let capability_observations =
        provider_api_capability_observations(&listed_routes, &listed.models, observed_at)
            .map_err(|error| (fetching.revision, error))?;
    if listed.provenance.source != ModelRecordSource::ProviderApi {
        return Err((
            fetching.revision,
            CoreError::new(
                CoreErrorCode::ProviderUnavailable,
                "provider model list contained unsupported provenance",
                false,
            ),
        ));
    }
    let review = ModelSyncReview::new(ModelSyncDiff {
        connection_id: task.connection_id.clone(),
        expected_connection: connection,
        expected_model_routes: existing_routes,
        observed_at,
        listed_routes,
        newly_seen_model_route_ids,
        missing_model_route_ids,
        initial_presets,
        capability_observations,
        routes_requiring_preset_configuration,
        provenance: ModelSyncSourceProvenance {
            source: model_record_source_name(listed.provenance.source).to_owned(),
            api_family: listed.provenance.api_family,
            api_origin: listed.provenance.api_origin,
            endpoint_path: listed.provenance.endpoint_path,
            pages_fetched: listed.pages_fetched,
            response_bytes: listed.response_bytes,
        },
    })
    .map_err(|message| (fetching.revision, CoreError::internal(message)))?;
    task.storage
        .store_model_sync_review(&task.job_id, fetching.revision, &review)
        .map_err(|error| (fetching.revision, error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier, Mutex, mpsc},
        thread,
        time::{Duration, Instant},
    };

    use async_trait::async_trait;
    use lorepia_domain::{
        CoreErrorCode, ModelSyncState, ProviderConnection, ProviderConnectionId, ProviderProfile,
    };
    use lorepia_providers::{ModelListResult, ModelListSupport};
    use tempfile::tempdir;
    use tokio::sync::oneshot;

    use super::*;
    use crate::CoreConfig;
    use lorepia_storage::{ProviderCredentialObservedStatus, ProviderCredentialOperationKind};

    fn seed_credential_connection(core: &Core, id: &str, base_url: String) -> ProviderConnection {
        core.upsert_provider_profile(ProviderProfile {
            id: id.to_owned(),
            display_name: "Model sync authority fixture".to_owned(),
            base_url,
            model: "fixture-model".to_owned(),
            timeout_seconds: 5,
        })
        .expect("seed model-sync provider profile");
        core.inner
            .storage
            .get_provider_connection(&ProviderConnectionId::from(id))
            .expect("read model-sync provider connection")
    }

    fn install_credential_access_authority(
        core: &Core,
        connection_id: &ProviderConnectionId,
    ) -> ProviderCredentialAccessAuthority {
        let authority = core
            .inner
            .storage
            .propose_provider_credential_install_authority(connection_id)
            .expect("propose model-sync credential install authority");
        let install = core
            .inner
            .storage
            .prepare_provider_credential_operation_with_install_authority(
                connection_id,
                ProviderCredentialOperationKind::Install,
                ProviderCredentialObservedStatus::Missing,
                Some(&authority),
            )
            .expect("prepare model-sync credential install");
        core.inner
            .storage
            .start_provider_credential_operation(&install.plan.operation_id, &install.plan_sha256)
            .expect("start model-sync credential install");
        core.inner
            .storage
            .finish_provider_credential_operation(
                &install.plan.operation_id,
                &install.plan_sha256,
                ProviderCredentialObservedStatus::Available,
            )
            .expect("finish model-sync credential install");
        core.inner
            .storage
            .ensure_provider_credential_access_settled(connection_id)
            .expect("read model-sync credential access authority")
    }

    fn terminally_remove_credential(core: &Core, connection_id: &ProviderConnectionId) {
        let removal = core
            .inner
            .storage
            .prepare_provider_credential_operation(
                connection_id,
                ProviderCredentialOperationKind::RemoveCredential,
                ProviderCredentialObservedStatus::Available,
            )
            .expect("prepare model-sync credential removal");
        core.inner
            .storage
            .start_provider_credential_operation(&removal.plan.operation_id, &removal.plan_sha256)
            .expect("start model-sync credential removal");
        core.inner
            .storage
            .finish_provider_credential_operation(
                &removal.plan.operation_id,
                &removal.plan_sha256,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("finish model-sync credential removal");
    }

    #[test]
    fn terminal_removal_rejects_cached_model_sync_before_provider_work() {
        let root = tempdir().expect("temporary core root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let connection = seed_credential_connection(
            &core,
            "stale-model-sync",
            "https://api.example.com/v1".to_owned(),
        );
        let cached_authority = install_credential_access_authority(&core, &connection.id);
        let cached_credential = ConnectionBoundCredential::new_with_access_authority(
            connection.id.clone(),
            Some("cached-model-sync-secret".to_owned()),
            cached_authority,
        );
        terminally_remove_credential(&core, &connection.id);

        let error = core
            .start_provider_model_sync_with_credential(&connection.id, cached_credential)
            .expect_err("removed credential authority must reject model sync");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert_eq!(core.inner.active_model_syncs.len(), 0);
        assert!(
            core.list_provider_model_syncs(&connection.id, 4)
                .expect("list model-sync jobs after stale authority")
                .is_empty()
        );
    }

    const PROVIDER_ENTERED: &str = "provider_entered";
    const PROVIDER_FUTURE_DROPPED: &str = "provider_future_dropped";
    const CREDENTIAL_DROPPED: &str = "credential_dropped";

    struct ProviderFutureDropProbe {
        lifecycle: mpsc::Sender<&'static str>,
    }

    impl Drop for ProviderFutureDropProbe {
        fn drop(&mut self) {
            let _ = self.lifecycle.send(PROVIDER_FUTURE_DROPPED);
        }
    }

    struct BlockingModelListing {
        lifecycle: mpsc::Sender<&'static str>,
        release: Mutex<Option<oneshot::Receiver<()>>>,
    }

    #[async_trait]
    impl ModelListing for BlockingModelListing {
        fn support(&self) -> ModelListSupport {
            ModelListSupport::Supported
        }

        async fn list_models(&self, request: ModelListRequest<'_>) -> CoreResult<ModelListResult> {
            let _future_drop = ProviderFutureDropProbe {
                lifecycle: self.lifecycle.clone(),
            };
            let release = {
                self.release
                    .lock()
                    .expect("blocking model listing lock")
                    .take()
                    .expect("one blocking model listing invocation")
            };
            self.lifecycle
                .send(PROVIDER_ENTERED)
                .expect("report model listing entry");
            release.await.expect("release blocking model listing");
            std::hint::black_box(request);
            Err(CoreError::new(
                CoreErrorCode::Cancelled,
                "operation was cancelled",
                true,
            ))
        }
    }

    struct CredentialDropGate {
        lifecycle: mpsc::Sender<&'static str>,
        release: Mutex<Option<mpsc::Receiver<()>>>,
    }

    impl Drop for CredentialDropGate {
        fn drop(&mut self) {
            let _ = self.lifecycle.send(CREDENTIAL_DROPPED);
            if let Some(release) = self
                .release
                .lock()
                .expect("credential drop gate lock")
                .take()
            {
                let _ = release.recv();
            }
        }
    }

    fn wait_for_cancellation_request(core: &Core, job_id: &ModelSyncJobId) {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if core
                .inner
                .active_model_syncs
                .cancellation_requested(job_id)
                .expect("read model-sync cancellation request")
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "model-sync cancellation request timed out"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn prepare_credential_removal(
        core: &Core,
        connection_id: &ProviderConnectionId,
        kind: ProviderCredentialOperationKind,
    ) -> CoreResult<lorepia_storage::StoredProviderCredentialOperation> {
        core.prepare_provider_credential_operation(
            connection_id,
            kind,
            ProviderCredentialObservedStatus::Available,
        )
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the production lifecycle regression records each teardown cutpoint in order"
    )]
    fn assert_cancel_blocks_credential_operation_until_teardown(
        kind: ProviderCredentialOperationKind,
        connection_suffix: &str,
    ) {
        let root = tempdir().expect("temporary core root");
        let core = Arc::new(Core::open(CoreConfig::new(root.path())).expect("open core"));
        let connection = seed_credential_connection(
            &core,
            &format!("cancel-model-sync-{connection_suffix}"),
            "https://api.example.com/v1".to_owned(),
        );
        let authority = install_credential_access_authority(&core, &connection.id);
        let (lifecycle_sender, lifecycle_receiver) = mpsc::channel();
        let (provider_release_sender, provider_release_receiver) = oneshot::channel();
        core.inner
            .active_model_syncs
            .install_model_listing_override(
                connection.id.clone(),
                Arc::new(BlockingModelListing {
                    lifecycle: lifecycle_sender.clone(),
                    release: Mutex::new(Some(provider_release_receiver)),
                }),
            )
            .expect("install blocking model listing");

        let (credential_release_sender, credential_release_receiver) = mpsc::channel();
        let credential = ConnectionBoundCredential::new_with_access_authority(
            connection.id.clone(),
            Some("synthetic-model-sync-secret".to_owned()),
            authority,
        )
        .with_dispatch_lease(CredentialDropGate {
            lifecycle: lifecycle_sender,
            release: Mutex::new(Some(credential_release_receiver)),
        });

        let (start_result_sender, start_result_receiver) = mpsc::sync_channel(1);
        let start_core = Arc::clone(&core);
        let start_connection_id = connection.id.clone();
        let start_worker =
            thread::spawn(move || {
                start_result_sender
                    .send(start_core.start_provider_model_sync_with_credential(
                        &start_connection_id,
                        credential,
                    ))
                    .expect("report production model-sync start");
            });

        let first_lifecycle = lifecycle_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("provider or credential lifecycle begins");
        if first_lifecycle != PROVIDER_ENTERED {
            credential_release_sender
                .send(())
                .expect("release prematurely dropped credential carrier");
            let _job_id = start_result_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("production model-sync start returns after early carrier drop")
                .expect("start production model sync");
            provider_release_sender
                .send(())
                .expect("release provider after early carrier drop");
            start_worker.join().expect("join production start worker");
            panic!(
                "credential carrier dropped before the production provider future started: \
                 {first_lifecycle}"
            );
        }

        let job_id = start_result_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("production model-sync start returns")
            .expect("start production model sync");
        start_worker.join().expect("join production start worker");
        assert!(
            lifecycle_receiver.try_recv().is_err(),
            "credential must remain owned while the provider future is blocked"
        );

        let cancel_start = Arc::new(Barrier::new(3));
        let (cancel_result_sender, cancel_result_receiver) = mpsc::channel();
        let cancel_workers = (0..2)
            .map(|index| {
                let cancel_core = Arc::clone(&core);
                let cancel_job_id = job_id.clone();
                let cancel_start = Arc::clone(&cancel_start);
                let cancel_result_sender = cancel_result_sender.clone();
                thread::spawn(move || {
                    cancel_start.wait();
                    cancel_result_sender
                        .send((
                            index,
                            cancel_core.cancel_provider_model_sync(&cancel_job_id),
                        ))
                        .expect("report concurrent production cancellation");
                })
            })
            .collect::<Vec<_>>();
        cancel_start.wait();
        wait_for_cancellation_request(&core, &job_id);
        assert!(
            cancel_result_receiver.try_recv().is_err(),
            "cancellation must wait for the blocked provider future"
        );
        assert_eq!(
            core.get_provider_model_sync(&job_id)
                .expect("read fetching model-sync job")
                .state,
            ModelSyncState::Fetching
        );
        let blocked = prepare_credential_removal(&core, &connection.id, kind)
            .expect_err("credential mutation must remain blocked during provider teardown");
        assert_eq!(blocked.code, CoreErrorCode::InvalidInput);

        provider_release_sender
            .send(())
            .expect("release blocking model listing");
        assert_eq!(
            lifecycle_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("provider future drops"),
            PROVIDER_FUTURE_DROPPED
        );
        assert_eq!(
            lifecycle_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("credential carrier starts dropping"),
            CREDENTIAL_DROPPED
        );
        assert!(
            cancel_result_receiver.try_recv().is_err(),
            "cancellation must wait for credential teardown and its terminal write"
        );
        let still_blocked = prepare_credential_removal(&core, &connection.id, kind)
            .expect_err("credential mutation must remain blocked until teardown is terminal");
        assert_eq!(still_blocked.code, CoreErrorCode::InvalidInput);

        credential_release_sender
            .send(())
            .expect("finish credential carrier drop");
        for _ in 0..2 {
            let (_index, cancelled) = cancel_result_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("concurrent cancellation completes after teardown");
            assert_eq!(
                cancelled
                    .expect("concurrent cancellation is idempotent")
                    .state,
                ModelSyncState::Cancelled
            );
        }
        for worker in cancel_workers {
            worker.join().expect("join concurrent cancellation");
        }
        let removal = prepare_credential_removal(&core, &connection.id, kind)
            .expect("credential mutation starts only after production teardown");
        assert_eq!(removal.plan.connection_id, connection.id);
        assert_eq!(
            core.get_provider_model_sync(&job_id)
                .expect("read terminal model-sync job")
                .state,
            ModelSyncState::Cancelled
        );
    }

    #[test]
    fn cancel_waits_for_stalled_provider_teardown_before_credential_removal() {
        assert_cancel_blocks_credential_operation_until_teardown(
            ProviderCredentialOperationKind::RemoveCredential,
            "remove",
        );
        assert_cancel_blocks_credential_operation_until_teardown(
            ProviderCredentialOperationKind::RemoveForArchive,
            "archive",
        );
    }
}
