use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    CoreError, CoreErrorCode, CoreResult, MODEL_SYNC_EVENT_VERSION, MODEL_SYNC_REDACTION_VERSION,
    ModelMetadataSource, ModelSyncEvent, ModelSyncFailure, ModelSyncJob, ModelSyncJobId,
    ModelSyncProgress, ModelSyncReview, ModelSyncState, ProviderConnection, ProviderConnectionId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::database::{
    Storage, decode_provider_connection_row, load_model_routes_for_reconciliation,
    provider_connection_columns, storage_db_error, upsert_capability_observation_row,
    upsert_generation_preset_row, upsert_model_route_row, upsert_provider_connection_row,
    validate_provider_api_snapshot_observation, validate_provider_catalog_foreign_keys,
};
use crate::{
    ProviderCredentialAccessAuthority,
    generation_attempt::validate_provider_credential_access_authority_in_transaction,
};

const MAX_OUTBOX_POLL: u32 = 512;
const MAX_JOB_LIST: u32 = 256;
const MAX_CONNECTION_SNAPSHOT_BYTES: usize = 64 * 1024;
const MAX_REVIEW_BYTES: usize = 8 * 1024 * 1024;
const MAX_FAILURE_BYTES: usize = 1024;
const MAX_EVENT_BYTES: usize = 16 * 1024;

#[derive(Debug)]
struct StoredModelSyncJob {
    public: ModelSyncJob,
    expected_connection: ProviderConnection,
    base_graph_sha256: String,
    approved_review_sha256: Option<String>,
}

type ModelSyncJobRow = (
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
);

fn ensure_provider_credential_operation_settled_for_model_sync(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<()> {
    let unresolved_exists = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM provider_credential_operations
                WHERE connection_id = ?1
                  AND status IN (
                    'prepared', 'started', 'cleanup_required', 'outcome_unknown'
                  )
             )",
            [connection_id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if unresolved_exists {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider connection cannot start model synchronization while its credential operation is unresolved",
            true,
        ));
    }
    Ok(())
}

impl Storage {
    /// Creates one durable, secret-free synchronization job and its first
    /// outbox event. A connection may have only one active job.
    pub fn create_model_sync_job(
        &self,
        expected_connection: &ProviderConnection,
    ) -> CoreResult<ModelSyncJob> {
        self.create_model_sync_job_inner(expected_connection, None, false)
    }

    /// Creates a synchronization job only if the supplied credential access
    /// authority is still the exact durable owner of the connection's native
    /// credential slot.
    ///
    /// Authority validation and job insertion share one `BEGIN IMMEDIATE`
    /// transaction, so a completed credential removal cannot be interleaved
    /// between the validation and the durable provider-work barrier.
    pub fn create_model_sync_job_with_credential_access_authority(
        &self,
        expected_connection: &ProviderConnection,
        credential_access_authority: Option<&ProviderCredentialAccessAuthority>,
    ) -> CoreResult<ModelSyncJob> {
        self.create_model_sync_job_inner(expected_connection, credential_access_authority, true)
    }

    fn create_model_sync_job_inner(
        &self,
        expected_connection: &ProviderConnection,
        credential_access_authority: Option<&ProviderCredentialAccessAuthority>,
        require_exact_credential_access_authority: bool,
    ) -> CoreResult<ModelSyncJob> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        if require_exact_credential_access_authority {
            validate_provider_credential_access_authority_in_transaction(
                &transaction,
                &expected_connection.id,
                credential_access_authority,
            )?;
        } else {
            ensure_provider_credential_operation_settled_for_model_sync(
                &transaction,
                &expected_connection.id,
            )?;
        }
        let active_exists = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM model_sync_jobs
                    WHERE connection_id = ?1
                      AND state IN (
                        'created',
                        'fetching',
                        'diff-ready-awaiting-review',
                        'committing'
                      )
                 )",
                [expected_connection.id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if active_exists {
            return Err(CoreError::invalid(
                "provider connection already has an active model synchronization",
            ));
        }
        let stored_connection = load_provider_connection(&transaction, &expected_connection.id)?;
        if stored_connection != *expected_connection {
            return Err(CoreError::invalid(
                "provider connection changed before model synchronization started",
            ));
        }

        let id = ModelSyncJobId::new();
        let now = Utc::now();
        let expected_connection_json =
            canonical_connection_json(expected_connection).map_err(CoreError::internal)?;
        let expected_connection_sha256 =
            hex::encode(Sha256::digest(expected_connection_json.as_bytes()));
        let base_graph_sha256 = model_sync_graph_sha256(&transaction, &expected_connection.id)?;
        transaction
            .execute(
                "INSERT INTO model_sync_jobs
                 (id, connection_id, state, revision, next_event_sequence,
                  expected_connection_json, expected_connection_sha256, base_graph_sha256,
                  review_json, review_sha256, approved_review_sha256, approved_at,
                  failure_json, created_at, updated_at)
                 VALUES (?1, ?2, 'created', 1, 2, ?3, ?4, ?5, NULL, NULL, NULL, NULL,
                         NULL, ?6, ?6)",
                params![
                    id.as_str(),
                    expected_connection.id.as_str(),
                    expected_connection_json,
                    expected_connection_sha256,
                    base_graph_sha256,
                    now.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        insert_model_sync_event(
            &transaction,
            &id,
            1,
            1,
            ModelSyncState::Created,
            progress(0, "model_sync.created"),
            None,
            None,
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_model_sync_job(&id)
    }

    pub fn get_model_sync_job(&self, id: &ModelSyncJobId) -> CoreResult<ModelSyncJob> {
        let connection = self.connection()?;
        Ok(load_model_sync_job(&connection, id)?.public)
    }

    /// Lists durable jobs newest-first so a restarted native client can
    /// rediscover review-ready or interrupted work without retaining an ID.
    pub fn list_model_sync_jobs(
        &self,
        connection_id: &ProviderConnectionId,
        limit: u32,
    ) -> CoreResult<Vec<ModelSyncJob>> {
        if limit == 0 || limit > MAX_JOB_LIST {
            return Err(CoreError::invalid(format!(
                "model synchronization job limit must be between 1 and {MAX_JOB_LIST}",
            )));
        }
        let connection = self.connection()?;
        let connection_exists = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM provider_connections WHERE id = ?1
                 )",
                [connection_id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !connection_exists {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "provider connection was not found",
                false,
            ));
        }
        let ids = {
            let mut statement = connection
                .prepare(
                    "SELECT id FROM model_sync_jobs
                     WHERE connection_id = ?1
                     ORDER BY created_at DESC, id DESC
                     LIMIT ?2",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(params![connection_id.as_str(), limit], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        ids.into_iter()
            .map(|id| load_model_sync_job(&connection, &ModelSyncJobId::from(id)))
            .map(|result| result.map(|stored| stored.public))
            .collect()
    }

    pub fn transition_model_sync_job_to_fetching(
        &self,
        id: &ModelSyncJobId,
        expected_revision: u64,
    ) -> CoreResult<ModelSyncJob> {
        self.transition_model_sync_job(
            id,
            expected_revision,
            ModelSyncState::Created,
            ModelSyncState::Fetching,
            progress(1, "model_sync.fetching"),
            None,
            None,
        )
    }

    pub fn store_model_sync_review(
        &self,
        id: &ModelSyncJobId,
        expected_revision: u64,
        review: &ModelSyncReview,
    ) -> CoreResult<ModelSyncJob> {
        review.verify().map_err(CoreError::invalid)?;
        validate_model_sync_review(id, review)?;
        if review.diff.connection_id != review.diff.expected_connection.id {
            return Err(CoreError::invalid(
                "model synchronization review connection is inconsistent",
            ));
        }
        let review_json = serde_json::to_string(review).map_err(|error| {
            CoreError::internal(format!(
                "cannot encode model synchronization review: {error}"
            ))
        })?;
        ensure_json_size(
            &review_json,
            MAX_REVIEW_BYTES,
            "model synchronization review",
        )?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let stored = load_model_sync_job(&transaction, id)?;
        require_state_revision(&stored.public, ModelSyncState::Fetching, expected_revision)?;
        if stored.expected_connection != review.diff.expected_connection {
            return Err(CoreError::invalid(
                "provider connection snapshot differs from model synchronization review",
            ));
        }
        let next_revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("model synchronization revision overflow"))?;
        let now = Utc::now();
        let changed = transaction
            .execute(
                "UPDATE model_sync_jobs
                 SET state = 'diff-ready-awaiting-review', revision = ?3,
                     review_json = ?4, review_sha256 = ?5, failure_json = NULL,
                     updated_at = ?6
                 WHERE id = ?1 AND state = 'fetching' AND revision = ?2",
                params![
                    id.as_str(),
                    expected_revision,
                    next_revision,
                    review_json,
                    review.sha256,
                    now.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        ensure_single_cas_update(changed)?;
        append_model_sync_event(
            &transaction,
            id,
            ModelSyncState::DiffReadyAwaitingReview,
            progress(2, "model_sync.awaiting_review"),
            Some(review.sha256.clone()),
            None,
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_model_sync_job(id)
    }

    /// Verifies the stored canonical diff and performs the review-state CAS.
    pub fn mark_model_sync_job_committing(
        &self,
        id: &ModelSyncJobId,
        expected_revision: u64,
        approved_review_sha256: &str,
    ) -> CoreResult<ModelSyncJob> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let stored = load_model_sync_job(&transaction, id)?;
        if stored.public.state == ModelSyncState::Completed {
            if stored
                .public
                .review
                .as_ref()
                .is_some_and(|review| review.sha256 == approved_review_sha256)
            {
                return Ok(stored.public);
            }
            return Err(CoreError::invalid(
                "approved model synchronization hash does not match the completed review",
            ));
        }
        require_state_revision(
            &stored.public,
            ModelSyncState::DiffReadyAwaitingReview,
            expected_revision,
        )?;
        let review = stored.public.review.as_ref().ok_or_else(|| {
            corrupted("review-ready model synchronization job has no stored review")
        })?;
        review.verify().map_err(corrupted)?;
        validate_model_sync_review(id, review).map_err(|error| corrupted(error.message))?;
        if review.sha256 != approved_review_sha256 {
            return Err(CoreError::invalid(
                "approved model synchronization hash does not match the current review",
            ));
        }
        let next_revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("model synchronization revision overflow"))?;
        let now = Utc::now();
        let changed = transaction
            .execute(
                "UPDATE model_sync_jobs
                 SET state = 'committing', revision = ?3,
                     approved_review_sha256 = ?4, approved_at = ?5, updated_at = ?5
                 WHERE id = ?1
                   AND state = 'diff-ready-awaiting-review'
                   AND revision = ?2
                   AND review_sha256 = ?4",
                params![
                    id.as_str(),
                    expected_revision,
                    next_revision,
                    approved_review_sha256,
                    now.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        ensure_single_cas_update(changed)?;
        append_model_sync_event(
            &transaction,
            id,
            ModelSyncState::Committing,
            progress(3, "model_sync.committing"),
            Some(approved_review_sha256.to_owned()),
            None,
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_model_sync_job(id)
    }

    /// Atomically applies the approved graph and completes the job.
    ///
    /// Any validation or write error rolls back routes, presets, observations,
    /// connection status, job state, and completion event together.
    #[allow(clippy::too_many_lines)]
    pub fn commit_model_sync_job(
        &self,
        id: &ModelSyncJobId,
        expected_revision: u64,
    ) -> CoreResult<ModelSyncJob> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let stored = load_model_sync_job(&transaction, id)?;
        if stored.public.state == ModelSyncState::Completed {
            return Ok(stored.public);
        }
        require_state_revision(
            &stored.public,
            ModelSyncState::Committing,
            expected_revision,
        )?;
        let review = stored.public.review.as_ref().ok_or_else(|| {
            corrupted("committing model synchronization job has no stored review")
        })?;
        review.verify().map_err(corrupted)?;
        validate_model_sync_review(id, review).map_err(|error| corrupted(error.message))?;
        if stored.approved_review_sha256.as_deref() != Some(review.sha256.as_str()) {
            return Err(corrupted(
                "committing model synchronization approval does not match its review",
            ));
        }
        let current_connection =
            load_provider_connection(&transaction, &stored.public.connection_id)?;
        if current_connection != stored.expected_connection
            || current_connection != review.diff.expected_connection
        {
            return Err(CoreError::invalid(
                "provider connection changed after model synchronization review",
            ));
        }
        let current_graph_sha256 =
            model_sync_graph_sha256(&transaction, &stored.public.connection_id)?;
        if current_graph_sha256 != stored.base_graph_sha256 {
            return Err(CoreError::invalid(
                "provider model graph changed after synchronization started",
            ));
        }
        let mut current_routes =
            load_model_routes_for_reconciliation(&transaction, &stored.public.connection_id)?;
        current_routes.sort_by(|left, right| left.id.cmp(&right.id));
        let mut expected_routes = review.diff.expected_model_routes.clone();
        expected_routes.sort_by(|left, right| left.id.cmp(&right.id));
        if current_routes != expected_routes {
            return Err(CoreError::invalid(
                "model route graph changed after model synchronization review",
            ));
        }

        let mut listed_ids = BTreeSet::new();
        for route in &review.diff.listed_routes {
            if route.connection_id != stored.public.connection_id {
                return Err(CoreError::invalid(
                    "reviewed model route belongs to a different provider connection",
                ));
            }
            if !listed_ids.insert(route.id.clone()) {
                return Err(CoreError::invalid(
                    "reviewed model route identifiers must be unique",
                ));
            }
            if route.miss_count != 0 {
                return Err(CoreError::invalid(
                    "listed model routes must reset their miss count",
                ));
            }
            upsert_model_route_row(&transaction, route)?;
        }
        let calculated_missing = expected_routes
            .iter()
            .filter(|route| !listed_ids.contains(&route.id))
            .map(|route| route.id.clone())
            .collect::<BTreeSet<_>>();
        let reviewed_missing = review
            .diff
            .missing_model_route_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if calculated_missing != reviewed_missing {
            return Err(CoreError::invalid(
                "reviewed missing-route set does not match the route graph",
            ));
        }
        for missing_id in calculated_missing {
            transaction
                .execute(
                    "UPDATE provider_models
                     SET miss_count = MIN(miss_count + 1, 4294967295),
                         last_reconciled_sync_job_id = ?3,
                         availability = CASE
                           WHEN availability IN (
                             'documented_only', 'access_denied', 'deprecated', 'retired'
                           ) THEN availability
                           ELSE 'missing_temporarily'
                         END
                     WHERE id = ?1 AND connection_id = ?2",
                    params![
                        missing_id.as_str(),
                        stored.public.connection_id.as_str(),
                        id.as_str()
                    ],
                )
                .map_err(storage_db_error)?;
        }
        for preset in &review.diff.initial_presets {
            if !listed_ids.contains(&preset.model_route_id) {
                return Err(CoreError::invalid(
                    "reviewed initial preset does not belong to a listed route",
                ));
            }
            upsert_generation_preset_row(&transaction, preset)?;
        }
        for listed_id in &listed_ids {
            transaction
                .execute(
                    "DELETE FROM model_capability_observations
                     WHERE model_route_id = ?1 AND source_kind = 'provider_api'",
                    [listed_id.as_str()],
                )
                .map_err(storage_db_error)?;
        }
        for observation in &review.diff.capability_observations {
            if !listed_ids.contains(&observation.model_route_id) {
                return Err(CoreError::invalid(
                    "reviewed capability observation does not belong to a listed route",
                ));
            }
            upsert_capability_observation_row(&transaction, observation)?;
        }
        let mut refreshed_connection = current_connection;
        refreshed_connection.status = lorepia_domain::ConnectionStatus::Connected;
        refreshed_connection.updated_at = review.diff.observed_at;
        upsert_provider_connection_row(&transaction, &refreshed_connection)?;
        validate_provider_catalog_foreign_keys(&transaction)?;

        let next_revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("model synchronization revision overflow"))?;
        let now = Utc::now();
        let changed = transaction
            .execute(
                "UPDATE model_sync_jobs
                 SET state = 'completed', revision = ?3, updated_at = ?4
                 WHERE id = ?1 AND state = 'committing' AND revision = ?2",
                params![
                    id.as_str(),
                    expected_revision,
                    next_revision,
                    now.to_rfc3339()
                ],
            )
            .map_err(storage_db_error)?;
        ensure_single_cas_update(changed)?;
        append_model_sync_event(
            &transaction,
            id,
            ModelSyncState::Completed,
            progress(4, "model_sync.completed"),
            Some(review.sha256.clone()),
            None,
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_model_sync_job(id)
    }

    pub fn fail_model_sync_job(
        &self,
        id: &ModelSyncJobId,
        expected_revision: u64,
        failure: &ModelSyncFailure,
    ) -> CoreResult<ModelSyncJob> {
        validate_failure(failure)?;
        let current = self.get_model_sync_job(id)?;
        if current.state.is_terminal() {
            return Ok(current);
        }
        self.transition_model_sync_job(
            id,
            expected_revision,
            current.state,
            ModelSyncState::Failed,
            progress(4, "model_sync.failed"),
            None,
            Some(failure.clone()),
        )
    }

    pub fn cancel_model_sync_job(&self, id: &ModelSyncJobId) -> CoreResult<ModelSyncJob> {
        let current = self.get_model_sync_job(id)?;
        if current.state == ModelSyncState::Cancelled {
            return Ok(current);
        }
        if current.state.is_terminal() {
            return Err(CoreError::invalid(
                "completed or failed model synchronization cannot be cancelled",
            ));
        }
        if current.state == ModelSyncState::Committing {
            return Err(CoreError::invalid(
                "model synchronization cannot be cancelled while committing",
            ));
        }
        self.transition_model_sync_job(
            id,
            current.revision,
            current.state,
            ModelSyncState::Cancelled,
            progress(4, "model_sync.cancelled"),
            None,
            None,
        )
    }

    /// Returns at most `limit` undelivered events for exactly one job.
    ///
    /// Polling is at-least-once: returned rows remain available until each
    /// `(job_id, sequence)` is acknowledged with
    /// [`Self::ack_model_sync_event`]. This prevents one job's consumer from
    /// consuming events that belong to another job.
    pub fn poll_model_sync_events_for_job(
        &self,
        id: &ModelSyncJobId,
        limit: u32,
    ) -> CoreResult<Vec<ModelSyncEvent>> {
        if limit == 0 || limit > MAX_OUTBOX_POLL {
            return Err(CoreError::invalid(format!(
                "model synchronization event limit must be between 1 and {MAX_OUTBOX_POLL}",
            )));
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        load_model_sync_job(&transaction, id)?;
        let rows = {
            let mut statement = transaction
                .prepare(
                    "SELECT sequence, event_json
                     FROM model_sync_event_outbox
                     WHERE job_id = ?1
                       AND delivered_at IS NULL
                       AND available_at <= ?3
                     ORDER BY sequence
                     LIMIT ?2",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(
                    params![id.as_str(), limit, Utc::now().to_rfc3339()],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        let mut events = Vec::with_capacity(rows.len());
        for (sequence, event_json) in rows {
            let sequence = u64::try_from(sequence)
                .map_err(|_| corrupted("stored model synchronization event sequence is invalid"))?;
            ensure_stored_json_size(
                &event_json,
                MAX_EVENT_BYTES,
                "model synchronization progress event",
            )?;
            let event = serde_json::from_str::<ModelSyncEvent>(&event_json).map_err(|error| {
                corrupted(format!(
                    "stored model synchronization event is invalid: {error}",
                ))
            })?;
            if event.job_id != *id || event.sequence != sequence {
                return Err(corrupted(
                    "stored model synchronization event identity differs from its outbox key",
                ));
            }
            let changed = transaction
                .execute(
                    "UPDATE model_sync_event_outbox
                     SET delivery_attempts = delivery_attempts + 1
                     WHERE job_id = ?1
                       AND sequence = ?2
                       AND delivered_at IS NULL",
                    params![id.as_str(), sequence],
                )
                .map_err(storage_db_error)?;
            if changed != 1 {
                return Err(CoreError::invalid(
                    "model synchronization event changed concurrently",
                ));
            }
            events.push(event);
        }
        transaction.commit().map_err(storage_db_error)?;
        Ok(events)
    }

    /// Acknowledges one previously polled event for exactly one job.
    ///
    /// Returns `false` when the event was already acknowledged or has not yet
    /// been polled. The composite identity prevents an acknowledgement for one
    /// job from consuming another job's same-numbered event.
    pub fn ack_model_sync_event(&self, id: &ModelSyncJobId, sequence: u64) -> CoreResult<bool> {
        if sequence == 0 {
            return Err(CoreError::invalid(
                "model synchronization event sequence must be positive",
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        load_model_sync_job(&transaction, id)?;
        let changed = transaction
            .execute(
                "UPDATE model_sync_event_outbox
                 SET delivered_at = ?3
                 WHERE job_id = ?1
                   AND sequence = ?2
                   AND delivered_at IS NULL
                   AND delivery_attempts > 0",
                params![id.as_str(), sequence, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(changed == 1)
    }

    #[allow(clippy::too_many_arguments)]
    fn transition_model_sync_job(
        &self,
        id: &ModelSyncJobId,
        expected_revision: u64,
        expected_state: ModelSyncState,
        next_state: ModelSyncState,
        event_progress: ModelSyncProgress,
        review_sha256: Option<String>,
        failure: Option<ModelSyncFailure>,
    ) -> CoreResult<ModelSyncJob> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let stored = load_model_sync_job(&transaction, id)?;
        require_state_revision(&stored.public, expected_state, expected_revision)?;
        let next_revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("model synchronization revision overflow"))?;
        let now = Utc::now();
        let failure_json = failure
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                CoreError::internal(format!(
                    "cannot encode model synchronization failure: {error}",
                ))
            })?;
        if let Some(failure_json) = failure_json.as_deref() {
            ensure_json_size(
                failure_json,
                MAX_FAILURE_BYTES,
                "model synchronization failure",
            )?;
        }
        let changed = transaction
            .execute(
                "UPDATE model_sync_jobs
                 SET state = ?3, revision = ?4, failure_json = ?5, updated_at = ?6
                 WHERE id = ?1 AND state = ?2 AND revision = ?7",
                params![
                    id.as_str(),
                    state_to_str(expected_state),
                    state_to_str(next_state),
                    next_revision,
                    failure_json,
                    now.to_rfc3339(),
                    expected_revision,
                ],
            )
            .map_err(storage_db_error)?;
        ensure_single_cas_update(changed)?;
        append_model_sync_event(
            &transaction,
            id,
            next_state,
            event_progress,
            review_sha256,
            failure,
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_model_sync_job(id)
    }
}

/// Converts abandoned network/commit work into an explicit user-visible state.
/// This function only mutates durable state; it never starts network work.
pub(crate) fn recover_interrupted_model_sync_jobs(connection: &mut Connection) -> CoreResult<()> {
    let transaction = connection.transaction().map_err(storage_db_error)?;
    let interrupted = {
        let mut statement = transaction
            .prepare(
                "SELECT id FROM model_sync_jobs
                 WHERE state IN ('created', 'fetching', 'committing')
                 ORDER BY created_at, id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    for id in interrupted {
        let id = ModelSyncJobId::from(id);
        let now = Utc::now();
        transaction
            .execute(
                "UPDATE model_sync_jobs
                 SET state = 'interrupted', revision = revision + 1, updated_at = ?2
                 WHERE id = ?1 AND state IN ('created', 'fetching', 'committing')",
                params![id.as_str(), now.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        append_model_sync_event(
            &transaction,
            &id,
            ModelSyncState::Interrupted,
            progress(0, "model_sync.interrupted"),
            None,
            None,
            now,
        )?;
    }
    transaction.commit().map_err(storage_db_error)
}

fn load_provider_connection(
    connection: &Connection,
    id: &ProviderConnectionId,
) -> CoreResult<ProviderConnection> {
    connection
        .query_row(
            "SELECT id, template_id, template_version, display_name, api_origin,
                    config_json, credential_ref, credential_scope_json, timeout_seconds,
                    status, created_at, updated_at
             FROM provider_connections
             WHERE id = ?1 AND archived_at IS NULL",
            [id.as_str()],
            provider_connection_columns,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider connection was not found",
                false,
            )
        })
        .and_then(decode_provider_connection_row)
}

fn load_model_sync_job(
    connection: &Connection,
    id: &ModelSyncJobId,
) -> CoreResult<StoredModelSyncJob> {
    let row = connection
        .query_row(
            "SELECT id, connection_id, state, revision,
                    expected_connection_json, expected_connection_sha256, base_graph_sha256,
                    review_json, review_sha256, approved_review_sha256,
                    failure_json, created_at, updated_at
             FROM model_sync_jobs WHERE id = ?1",
            [id.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "model synchronization job was not found",
                false,
            )
        })?;
    decode_model_sync_job(row)
}

fn decode_model_sync_job(row: ModelSyncJobRow) -> CoreResult<StoredModelSyncJob> {
    let (
        id,
        connection_id,
        state,
        revision,
        expected_connection_json,
        expected_connection_sha256,
        base_graph_sha256,
        review_json,
        review_sha256,
        approved_review_sha256,
        failure_json,
        created_at,
        updated_at,
    ) = row;
    let actual_connection_sha256 = hex::encode(Sha256::digest(expected_connection_json.as_bytes()));
    if actual_connection_sha256 != expected_connection_sha256 {
        return Err(corrupted(
            "stored model synchronization connection snapshot hash is invalid",
        ));
    }
    ensure_stored_json_size(
        &expected_connection_json,
        MAX_CONNECTION_SNAPSHOT_BYTES,
        "model synchronization connection snapshot",
    )?;
    let expected_connection = serde_json::from_str::<ProviderConnection>(&expected_connection_json)
        .map_err(|error| {
            corrupted(format!(
                "stored model synchronization connection snapshot is invalid: {error}",
            ))
        })?;
    let review = review_json
        .map(|value| {
            ensure_stored_json_size(&value, MAX_REVIEW_BYTES, "model synchronization review")?;
            serde_json::from_str::<ModelSyncReview>(&value).map_err(|error| {
                corrupted(format!(
                    "stored model synchronization review is invalid: {error}",
                ))
            })
        })
        .transpose()?;
    match (&review, &review_sha256) {
        (Some(review), Some(stored_sha256)) => {
            review.verify().map_err(corrupted)?;
            if &review.sha256 != stored_sha256 {
                return Err(corrupted(
                    "stored model synchronization review digest is inconsistent",
                ));
            }
        }
        (None, None) => {}
        _ => {
            return Err(corrupted(
                "stored model synchronization review columns are inconsistent",
            ));
        }
    }
    if let Some(review) = review.as_ref() {
        validate_model_sync_review(&ModelSyncJobId::from(id.clone()), review)
            .map_err(|error| corrupted(error.message))?;
    }
    let failure = failure_json
        .map(|value| {
            ensure_stored_json_size(&value, MAX_FAILURE_BYTES, "model synchronization failure")?;
            serde_json::from_str::<ModelSyncFailure>(&value).map_err(|error| {
                corrupted(format!(
                    "stored model synchronization failure is invalid: {error}",
                ))
            })
        })
        .transpose()?;
    if let Some(failure) = failure.as_ref() {
        validate_failure(failure).map_err(|error| corrupted(error.message))?;
    }
    let revision = u64::try_from(revision)
        .map_err(|_| corrupted("stored model synchronization revision is invalid"))?;
    Ok(StoredModelSyncJob {
        public: ModelSyncJob {
            id: ModelSyncJobId::from(id),
            connection_id: ProviderConnectionId::from(connection_id),
            state: str_to_state(&state)?,
            revision,
            review,
            failure,
            created_at: parse_datetime(&created_at, "created_at")?,
            updated_at: parse_datetime(&updated_at, "updated_at")?,
        },
        expected_connection,
        base_graph_sha256,
        approved_review_sha256,
    })
}

#[allow(clippy::too_many_arguments)]
fn insert_model_sync_event(
    transaction: &Transaction<'_>,
    id: &ModelSyncJobId,
    sequence: u64,
    job_revision: u64,
    state: ModelSyncState,
    progress: ModelSyncProgress,
    review_sha256: Option<String>,
    failure: Option<ModelSyncFailure>,
    emitted_at: DateTime<Utc>,
) -> CoreResult<()> {
    let event = ModelSyncEvent {
        version: MODEL_SYNC_EVENT_VERSION,
        job_id: id.clone(),
        sequence,
        job_revision,
        redaction_version: MODEL_SYNC_REDACTION_VERSION,
        state,
        progress,
        review_sha256,
        failure,
        emitted_at,
    };
    let event_json = serde_json::to_string(&event).map_err(|error| {
        CoreError::internal(format!(
            "cannot encode model synchronization progress event: {error}",
        ))
    })?;
    ensure_json_size(
        &event_json,
        MAX_EVENT_BYTES,
        "model synchronization progress event",
    )?;
    transaction
        .execute(
            "INSERT INTO model_sync_event_outbox
             (job_id, sequence, event_version, job_revision, state,
              redaction_version, event_json, created_at, available_at,
              delivery_attempts, delivered_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, 0, NULL)",
            params![
                id.as_str(),
                sequence,
                MODEL_SYNC_EVENT_VERSION,
                job_revision,
                state_to_str(state),
                MODEL_SYNC_REDACTION_VERSION,
                event_json,
                emitted_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn append_model_sync_event(
    transaction: &Transaction<'_>,
    id: &ModelSyncJobId,
    state: ModelSyncState,
    progress: ModelSyncProgress,
    review_sha256: Option<String>,
    failure: Option<ModelSyncFailure>,
    emitted_at: DateTime<Utc>,
) -> CoreResult<()> {
    let (sequence, job_revision) = transaction
        .query_row(
            "SELECT next_event_sequence, revision
             FROM model_sync_jobs WHERE id = ?1",
            [id.as_str()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(storage_db_error)?;
    let sequence = u64::try_from(sequence)
        .map_err(|_| corrupted("stored model synchronization event sequence is invalid"))?;
    let job_revision = u64::try_from(job_revision)
        .map_err(|_| corrupted("stored model synchronization event revision is invalid"))?;
    insert_model_sync_event(
        transaction,
        id,
        sequence,
        job_revision,
        state,
        progress,
        review_sha256,
        failure,
        emitted_at,
    )?;
    let changed = transaction
        .execute(
            "UPDATE model_sync_jobs
             SET next_event_sequence = next_event_sequence + 1
             WHERE id = ?1 AND next_event_sequence = ?2",
            params![id.as_str(), sequence],
        )
        .map_err(storage_db_error)?;
    ensure_single_cas_update(changed)
}

fn require_state_revision(
    job: &ModelSyncJob,
    state: ModelSyncState,
    revision: u64,
) -> CoreResult<()> {
    if job.state != state || job.revision != revision {
        return Err(CoreError::invalid(
            "model synchronization state changed concurrently",
        ));
    }
    Ok(())
}

fn ensure_single_cas_update(changed: usize) -> CoreResult<()> {
    if changed == 1 {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "model synchronization state changed concurrently",
        ))
    }
}

fn canonical_connection_json(connection: &ProviderConnection) -> Result<String, String> {
    let json = serde_json::to_string(connection)
        .map_err(|error| format!("cannot encode provider connection snapshot: {error}"))?;
    if json.len() > MAX_CONNECTION_SNAPSHOT_BYTES {
        return Err("provider connection snapshot exceeds its persistence bound".to_owned());
    }
    Ok(json)
}

fn model_sync_graph_sha256(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<String> {
    let provider_connection = load_provider_connection(connection, connection_id)?;
    let connection_json =
        canonical_connection_json(&provider_connection).map_err(CoreError::internal)?;
    let routes = query_graph_rows(
        connection,
        "SELECT id, connection_id, api_family, model_id, display_name, route_json,
                availability, raw_metadata_json, CAST(miss_count AS TEXT),
                metadata_source_kind, metadata_observed_at,
                last_reconciled_sync_job_id, metadata_sync_job_id,
                first_seen_at, last_seen_at
         FROM provider_models
         WHERE connection_id = ?1
         ORDER BY id",
        connection_id,
        15,
    )?;
    let presets = query_graph_rows(
        connection,
        "SELECT p.id, p.model_route_id, p.display_name, p.values_json,
                p.created_at, p.updated_at
         FROM generation_presets p
         JOIN provider_models m ON m.id = p.model_route_id
         WHERE m.connection_id = ?1
         ORDER BY p.id",
        connection_id,
        6,
    )?;
    let observations = query_graph_rows(
        connection,
        "SELECT o.id, o.model_route_id, o.capability_key, o.value_json,
                o.support_status, o.source_kind, o.confidence, o.evidence_ref,
                o.observed_at, o.expires_at
         FROM model_capability_observations o
         JOIN provider_models m ON m.id = o.model_route_id
         WHERE m.connection_id = ?1
         ORDER BY o.id",
        connection_id,
        10,
    )?;
    let canonical = serde_json::to_vec(&serde_json::json!({
        "connection": connection_json,
        "routes": routes,
        "presets": presets,
        "observations": observations,
    }))
    .map_err(|error| {
        CoreError::internal(format!(
            "cannot encode provider model graph snapshot: {error}",
        ))
    })?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

fn query_graph_rows(
    connection: &Connection,
    sql: &str,
    connection_id: &ProviderConnectionId,
    column_count: usize,
) -> CoreResult<Vec<Vec<Option<String>>>> {
    let mut statement = connection.prepare(sql).map_err(storage_db_error)?;
    statement
        .query_map([connection_id.as_str()], |row| {
            (0..column_count)
                .map(|index| row.get::<_, Option<String>>(index))
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn progress(completed_steps: u32, message_key: &str) -> ModelSyncProgress {
    ModelSyncProgress {
        completed_steps,
        total_steps: 4,
        message_key: message_key.to_owned(),
    }
}

fn ensure_json_size(json: &str, maximum: usize, label: &str) -> CoreResult<()> {
    if json.len() > maximum {
        return Err(CoreError::invalid(format!(
            "{label} exceeds its persistence bound",
        )));
    }
    Ok(())
}

fn ensure_stored_json_size(json: &str, maximum: usize, label: &str) -> CoreResult<()> {
    if json.len() > maximum {
        return Err(corrupted(format!("{label} exceeds its persistence bound")));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn validate_model_sync_review(job_id: &ModelSyncJobId, review: &ModelSyncReview) -> CoreResult<()> {
    let diff = &review.diff;
    if diff.connection_id != diff.expected_connection.id
        || diff.provenance.api_origin != diff.expected_connection.api_origin
        || diff.provenance.source != "provider_api"
    {
        return Err(CoreError::invalid(
            "model synchronization review provenance is inconsistent",
        ));
    }
    let mut expected_ids = BTreeSet::new();
    for route in &diff.expected_model_routes {
        if route.connection_id != diff.connection_id || !expected_ids.insert(route.id.clone()) {
            return Err(CoreError::invalid(
                "model synchronization base routes are inconsistent",
            ));
        }
    }
    let mut listed_ids = BTreeSet::new();
    for route in &diff.listed_routes {
        if route.connection_id != diff.connection_id
            || route.api_family != diff.provenance.api_family
            || !listed_ids.insert(route.id.clone())
            || route.miss_count != 0
            || route.metadata_source != ModelMetadataSource::ProviderApi
            || route.metadata_observed_at != Some(diff.observed_at)
            || route.last_seen_at != Some(diff.observed_at)
            || route.last_reconciled_sync_job_id.as_ref() != Some(job_id)
            || route.metadata_sync_job_id.as_ref() != Some(job_id)
        {
            return Err(CoreError::invalid(
                "listed model synchronization route is inconsistent",
            ));
        }
        validate_provider_api_route_metadata(route.raw_metadata.as_ref())?;
        if let Some(existing) = diff
            .expected_model_routes
            .iter()
            .find(|existing| existing.id == route.id)
        {
            if existing.connection_id != route.connection_id
                || existing.api_family != route.api_family
                || existing.model_id != route.model_id
                || existing.route_config != route.route_config
                || existing.display_name != route.display_name
                || existing.first_seen_at != route.first_seen_at
            {
                return Err(CoreError::invalid(
                    "model synchronization cannot rename or mutate a stable route identity",
                ));
            }
        } else if route.first_seen_at != diff.observed_at {
            return Err(CoreError::invalid(
                "new model synchronization route has an invalid first-seen time",
            ));
        }
    }

    let calculated_new = listed_ids
        .difference(&expected_ids)
        .cloned()
        .collect::<BTreeSet<_>>();
    let reviewed_new = diff
        .newly_seen_model_route_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if reviewed_new.len() != diff.newly_seen_model_route_ids.len() || reviewed_new != calculated_new
    {
        return Err(CoreError::invalid(
            "model synchronization new-route set is inconsistent",
        ));
    }
    let calculated_missing = expected_ids
        .difference(&listed_ids)
        .cloned()
        .collect::<BTreeSet<_>>();
    let reviewed_missing = diff
        .missing_model_route_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if reviewed_missing.len() != diff.missing_model_route_ids.len()
        || reviewed_missing != calculated_missing
    {
        return Err(CoreError::invalid(
            "model synchronization missing-route set is inconsistent",
        ));
    }

    let mut configured_new_routes = BTreeSet::new();
    let mut preset_ids = BTreeSet::new();
    for preset in &diff.initial_presets {
        if !calculated_new.contains(&preset.model_route_id)
            || !configured_new_routes.insert(preset.model_route_id.clone())
            || !preset_ids.insert(preset.id.clone())
        {
            return Err(CoreError::invalid(
                "model synchronization initial presets are inconsistent",
            ));
        }
    }
    let required_configuration = diff
        .routes_requiring_preset_configuration
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if required_configuration.len() != diff.routes_requiring_preset_configuration.len()
        || !configured_new_routes.is_disjoint(&required_configuration)
        || configured_new_routes
            .union(&required_configuration)
            .cloned()
            .collect::<BTreeSet<_>>()
            != calculated_new
    {
        return Err(CoreError::invalid(
            "model synchronization preset decision set is inconsistent",
        ));
    }
    let mut observation_ids = BTreeSet::new();
    for observation in &diff.capability_observations {
        if !listed_ids.contains(&observation.model_route_id)
            || !observation_ids.insert(observation.id.clone())
        {
            return Err(CoreError::invalid(
                "model synchronization capability observations are inconsistent",
            ));
        }
        validate_provider_api_snapshot_observation(observation, diff.observed_at)?;
    }
    Ok(())
}

/// Validate the canonical, bounded provider-API metadata projection stored on
/// a model route.
///
/// Both durable model synchronization and initial provider discovery use this
/// validator so neither publication path can persist arbitrary provider
/// response fields.
pub fn validate_provider_api_route_metadata(
    metadata: Option<&lorepia_domain::BoundedJson>,
) -> CoreResult<()> {
    let metadata = metadata.ok_or_else(|| {
        CoreError::invalid("listed model route must contain normalized provider metadata")
    })?;
    let value = serde_json::from_str::<serde_json::Value>(metadata.as_str())
        .map_err(|_| CoreError::invalid("listed model route metadata is invalid"))?;
    let object = value
        .as_object()
        .ok_or_else(|| CoreError::invalid("listed model route metadata must be an object"))?;
    let allowed = [
        "capabilities",
        "max_input_tokens",
        "max_output_tokens",
        "supported_generation_methods",
    ];
    if object.len() != allowed.len() || object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(CoreError::invalid(
            "listed model route metadata contains an unsupported field",
        ));
    }
    for key in ["max_input_tokens", "max_output_tokens"] {
        let value = &object[key];
        if !value.is_null() && value.as_u64().is_none_or(|value| value == 0) {
            return Err(CoreError::invalid(
                "listed model route token metadata is invalid",
            ));
        }
    }
    let methods = object["supported_generation_methods"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("listed model route generation methods are invalid"))?;
    if methods.len() > 128
        || methods.iter().any(|method| {
            method
                .as_str()
                .is_none_or(|method| method.is_empty() || method.len() > 256)
        })
    {
        return Err(CoreError::invalid(
            "listed model route generation methods are invalid",
        ));
    }
    validate_provider_api_capabilities(&object["capabilities"])
}

#[allow(clippy::too_many_lines)]
fn validate_provider_api_capabilities(value: &serde_json::Value) -> CoreResult<()> {
    const CAPABILITIES: &[&str] = &[
        "json_mode",
        "logprobs",
        "parallel_tool_calling",
        "reasoning",
        "seed",
        "structured_output",
        "tool_calling",
    ];
    const REASONING_FIELDS: &[&str] = &[
        "default_effort",
        "default_enabled",
        "mandatory",
        "supported_efforts",
        "supports_max_tokens",
    ];
    const EFFORTS: &[&str] = &["max", "xhigh", "high", "medium", "low", "minimal", "none"];
    let capabilities = value.as_object().ok_or_else(|| {
        CoreError::invalid("listed model route capabilities must be a normalized object")
    })?;
    let allowed = ["parameters", "reasoning", "supported"];
    if !(2..=allowed.len()).contains(&capabilities.len())
        || !capabilities.contains_key("reasoning")
        || !capabilities.contains_key("supported")
        || capabilities
            .keys()
            .any(|key| !allowed.contains(&key.as_str()))
    {
        return Err(CoreError::invalid(
            "listed model route capabilities contain an unsupported field",
        ));
    }
    let supported = capabilities["supported"].as_array().ok_or_else(|| {
        CoreError::invalid("listed model route supported capabilities are invalid")
    })?;
    if supported.len() > CAPABILITIES.len()
        || supported.iter().any(|capability| {
            capability
                .as_str()
                .is_none_or(|capability| !CAPABILITIES.contains(&capability))
        })
    {
        return Err(CoreError::invalid(
            "listed model route supported capabilities are invalid",
        ));
    }
    let supported_values = supported
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    if !supported_values.windows(2).all(|pair| {
        capability_index(pair[0])
            .is_some_and(|left| capability_index(pair[1]).is_some_and(|right| left < right))
    }) {
        return Err(CoreError::invalid(
            "listed model route supported capabilities are not canonical",
        ));
    }
    if let Some(parameters) = capabilities.get("parameters") {
        validate_provider_api_parameters(parameters)?;
        validate_provider_api_parameter_semantics(
            parameters,
            &supported_values,
            &capabilities["reasoning"],
        )?;
    }
    let reasoning = &capabilities["reasoning"];
    if reasoning.is_null() {
        return Ok(());
    }
    if !supported_values.contains(&"reasoning") {
        return Err(CoreError::invalid(
            "listed model route reasoning metadata lacks a capability claim",
        ));
    }
    let reasoning = reasoning
        .as_object()
        .ok_or_else(|| CoreError::invalid("listed model route reasoning metadata is invalid"))?;
    if reasoning.len() != REASONING_FIELDS.len()
        || reasoning
            .keys()
            .any(|key| !REASONING_FIELDS.contains(&key.as_str()))
    {
        return Err(CoreError::invalid(
            "listed model route reasoning metadata contains an unsupported field",
        ));
    }
    for key in ["default_enabled", "mandatory", "supports_max_tokens"] {
        if !reasoning[key].is_null() && !reasoning[key].is_boolean() {
            return Err(CoreError::invalid(
                "listed model route reasoning flags are invalid",
            ));
        }
    }
    let default_effort = reasoning["default_effort"].as_str();
    if !reasoning["default_effort"].is_null()
        && default_effort.is_none_or(|effort| !EFFORTS.contains(&effort))
    {
        return Err(CoreError::invalid(
            "listed model route reasoning default is invalid",
        ));
    }
    let effort_support = reasoning["supported_efforts"].as_object().ok_or_else(|| {
        CoreError::invalid("listed model route reasoning effort support is invalid")
    })?;
    let kind = effort_support
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CoreError::invalid("listed model route reasoning effort support is invalid")
        })?;
    let exact_efforts = match kind {
        "not_exposed" | "all_gateway" if effort_support.len() == 1 => None,
        "exact"
            if effort_support.len() == 2
                && effort_support.contains_key("values")
                && effort_support.contains_key("kind") =>
        {
            let values = effort_support["values"].as_array().ok_or_else(|| {
                CoreError::invalid("listed model route reasoning effort values are invalid")
            })?;
            if values.len() > EFFORTS.len()
                || values.iter().any(|effort| {
                    effort
                        .as_str()
                        .is_none_or(|effort| !EFFORTS.contains(&effort))
                })
            {
                return Err(CoreError::invalid(
                    "listed model route reasoning effort values are invalid",
                ));
            }
            let values = values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>();
            if values
                .windows(2)
                .any(|pair| effort_index(pair[0]) >= effort_index(pair[1]))
            {
                return Err(CoreError::invalid(
                    "listed model route reasoning effort values are not canonical",
                ));
            }
            Some(values)
        }
        _ => {
            return Err(CoreError::invalid(
                "listed model route reasoning effort support is invalid",
            ));
        }
    };
    let default_supported = match (default_effort, kind, exact_efforts.as_ref()) {
        (None, _, _) | (Some(_), "all_gateway", _) => true,
        (Some(default), "exact", Some(values)) => values.contains(&default),
        _ => false,
    };
    let supports_none = match (kind, exact_efforts.as_ref()) {
        ("all_gateway", _) => true,
        ("exact", Some(values)) => values.contains(&"none"),
        _ => false,
    };
    let mandatory = reasoning["mandatory"].as_bool() == Some(true);
    if !default_supported
        || mandatory
            && (supports_none
                || default_effort == Some("none")
                || reasoning["default_enabled"].as_bool() == Some(false))
        || default_effort == Some("none") && reasoning["default_enabled"].as_bool() == Some(true)
    {
        return Err(CoreError::invalid(
            "listed model route reasoning metadata is contradictory",
        ));
    }
    Ok(())
}

fn validate_provider_api_parameter_semantics(
    parameters: &serde_json::Value,
    supported_capabilities: &[&str],
    reasoning: &serde_json::Value,
) -> CoreResult<()> {
    if parameters.get("kind").and_then(serde_json::Value::as_str) != Some("exact") {
        return Ok(());
    }
    let values = parameters["values"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("listed model route parameter support is invalid"))?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<BTreeSet<_>>();
    let has = |parameter: &str| values.contains(parameter);
    if has("parallel_tool_calls") && !has("tools") {
        return Err(CoreError::invalid(
            "listed model route parallel tool support requires tools",
        ));
    }
    let mut expected = BTreeSet::new();
    if has("reasoning") || has("reasoning_effort") {
        expected.insert("reasoning");
    }
    if has("tools") {
        expected.insert("tool_calling");
    }
    if has("parallel_tool_calls") {
        expected.insert("parallel_tool_calling");
    }
    if has("structured_outputs") {
        expected.insert("structured_output");
    }
    if has("response_format") {
        expected.insert("json_mode");
    }
    if has("logprobs") {
        expected.insert("logprobs");
    }
    if has("seed") {
        expected.insert("seed");
    }
    let actual = supported_capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(CoreError::invalid(
            "listed model route capabilities contradict exact supported parameters",
        ));
    }
    if !reasoning.is_null() && !has("reasoning") && !has("reasoning_effort") {
        return Err(CoreError::invalid(
            "listed model route reasoning metadata lacks a supported reasoning parameter",
        ));
    }
    Ok(())
}

fn capability_index(capability: &str) -> Option<usize> {
    match capability {
        "reasoning" => Some(0),
        "tool_calling" => Some(1),
        "parallel_tool_calling" => Some(2),
        "structured_output" => Some(3),
        "json_mode" => Some(4),
        "logprobs" => Some(5),
        "seed" => Some(6),
        _ => None,
    }
}

fn validate_provider_api_parameters(value: &serde_json::Value) -> CoreResult<()> {
    let parameters = value
        .as_object()
        .ok_or_else(|| CoreError::invalid("listed model route parameter support is invalid"))?;
    let kind = parameters
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| CoreError::invalid("listed model route parameter support is invalid"))?;
    match kind {
        "not_exposed" if parameters.len() == 1 => Ok(()),
        "exact"
            if parameters.len() == 2
                && parameters.contains_key("kind")
                && parameters.contains_key("values") =>
        {
            const SUPPORTED_PARAMETERS: &[&str] = &[
                "frequency_penalty",
                "include_reasoning",
                "logprobs",
                "max_completion_tokens",
                "max_tokens",
                "parallel_tool_calls",
                "presence_penalty",
                "reasoning",
                "reasoning_effort",
                "response_format",
                "seed",
                "stop",
                "structured_outputs",
                "temperature",
                "tool_choice",
                "tools",
                "top_p",
            ];
            let values = parameters["values"].as_array().ok_or_else(|| {
                CoreError::invalid("listed model route parameter support values are invalid")
            })?;
            if values.len() > SUPPORTED_PARAMETERS.len()
                || values.iter().any(|parameter| {
                    parameter
                        .as_str()
                        .is_none_or(|parameter| !SUPPORTED_PARAMETERS.contains(&parameter))
                })
            {
                return Err(CoreError::invalid(
                    "listed model route parameter support values are invalid",
                ));
            }
            let values = values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>();
            if values.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(CoreError::invalid(
                    "listed model route parameter support values are not canonical",
                ));
            }
            Ok(())
        }
        _ => Err(CoreError::invalid(
            "listed model route parameter support is invalid",
        )),
    }
}

fn effort_index(effort: &str) -> usize {
    match effort {
        "max" => 0,
        "xhigh" => 1,
        "high" => 2,
        "medium" => 3,
        "low" => 4,
        "minimal" => 5,
        "none" => 6,
        _ => usize::MAX,
    }
}

fn validate_failure(failure: &ModelSyncFailure) -> CoreResult<()> {
    const ALLOWED_CODES: &[&str] = &[
        "invalid_input",
        "unsupported_content",
        "unsafe_archive",
        "not_found",
        "permission_denied",
        "storage_unavailable",
        "storage_corrupted",
        "provider_auth_failed",
        "provider_rate_limited",
        "provider_unavailable",
        "network_unavailable",
        "cancelled",
        "internal",
    ];
    if !ALLOWED_CODES.contains(&failure.code.as_str()) || failure.message_key != "model_sync.failed"
    {
        return Err(CoreError::invalid(
            "model synchronization failure must contain only a stable code and message key",
        ));
    }
    Ok(())
}

fn parse_datetime(value: &str, label: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            corrupted(format!(
                "stored model synchronization {label} is invalid: {error}",
            ))
        })
}

const fn state_to_str(state: ModelSyncState) -> &'static str {
    match state {
        ModelSyncState::Created => "created",
        ModelSyncState::Fetching => "fetching",
        ModelSyncState::Interrupted => "interrupted",
        ModelSyncState::DiffReadyAwaitingReview => "diff-ready-awaiting-review",
        ModelSyncState::Committing => "committing",
        ModelSyncState::Completed => "completed",
        ModelSyncState::Failed => "failed",
        ModelSyncState::Cancelled => "cancelled",
    }
}

fn str_to_state(value: &str) -> CoreResult<ModelSyncState> {
    match value {
        "created" => Ok(ModelSyncState::Created),
        "fetching" => Ok(ModelSyncState::Fetching),
        "interrupted" => Ok(ModelSyncState::Interrupted),
        "diff-ready-awaiting-review" => Ok(ModelSyncState::DiffReadyAwaitingReview),
        "committing" => Ok(ModelSyncState::Committing),
        "completed" => Ok(ModelSyncState::Completed),
        "failed" => Ok(ModelSyncState::Failed),
        "cancelled" => Ok(ModelSyncState::Cancelled),
        _ => Err(corrupted(format!(
            "stored model synchronization state is invalid: {value}",
        ))),
    }
}

fn corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use lorepia_domain::{
        ApiFamily, AppSettings, AuthBinding, BoundedJson, CanonicalOrigin, CapabilityKey,
        CapabilityObservation, CapabilityValue, Confidence, ConnectionConfig,
        ConnectionConfigEntry, ConnectionConfigValue, ConnectionStatus, CredentialRedirectPolicy,
        CredentialRef, CredentialScope, EndpointPath, GenerationPreset, GenerationPresetId,
        GenerationPromptCacheSettings, GenerationReasoningSettings, ModelAvailability,
        ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId, ModelSyncDiff,
        ModelSyncJobId, ModelSyncReview, ModelSyncSourceProvenance, ModelSyncState, ObservationId,
        ObservationSource, ProviderConnection, ProviderConnectionId, ProviderNetworkMode,
        ProviderProfile, ProviderTemplateId, SupportStatus,
    };
    use serde_json::json;
    use tempfile::tempdir;

    use crate::{ProviderCredentialObservedStatus, ProviderCredentialOperationKind};

    use super::{Storage, validate_provider_api_route_metadata};

    fn seeded_storage() -> (tempfile::TempDir, Storage, ProviderConnection, ModelRoute) {
        let root = tempdir().expect("temporary data root");
        let storage = Storage::open(root.path()).expect("open storage");
        storage
            .save_provider_profile(&ProviderProfile {
                id: "model-sync".to_owned(),
                display_name: "Model sync".to_owned(),
                base_url: "https://api.example.test/v1".to_owned(),
                model: "existing-model".to_owned(),
                timeout_seconds: 30,
            })
            .expect("seed provider graph");
        let connection = storage
            .get_provider_connection(&ProviderConnectionId::from("model-sync"))
            .expect("seeded connection");
        let route = storage
            .get_model_route(&ModelRouteId::from("model-sync"))
            .expect("seeded model route");
        (root, storage, connection, route)
    }

    fn seeded_modern_storage() -> (tempfile::TempDir, Storage, ProviderConnection, ModelRoute) {
        let root = tempdir().expect("temporary direct data root");
        let storage = Storage::open(root.path()).expect("open direct storage");
        let now = Utc::now();
        let connection_id = ProviderConnectionId::from("model-sync-modern");
        let api_origin = CanonicalOrigin::parse("https://api.example.test").expect("API origin");
        let connection = ProviderConnection {
            id: connection_id.clone(),
            template_id: ProviderTemplateId::from("custom-openai-chat-v1"),
            template_version: 1,
            display_name: "Modern model sync".to_owned(),
            api_origin: api_origin.clone(),
            config: ConnectionConfig {
                api_base_path: Some(EndpointPath::parse("/v1").expect("API base path")),
                network_mode: ProviderNetworkMode::Public,
                local_network_approval: None,
                values: vec![ConnectionConfigEntry {
                    key: "api_base_url".to_owned(),
                    value: ConnectionConfigValue::Text("https://api.example.test/v1".to_owned()),
                }],
            },
            credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
            credential_scope: Some(CredentialScope {
                allowed_origins: vec![api_origin],
                auth_binding: AuthBinding::BearerHeader,
                redirect_policy: CredentialRedirectPolicy::Deny,
            }),
            timeout_seconds: 30,
            status: ConnectionStatus::Untested,
            created_at: now,
            updated_at: now,
        };
        storage
            .save_provider_connection(&connection)
            .expect("save modern direct connection");
        let route = ModelRoute {
            id: ModelRouteId::from("model-sync-modern-route"),
            connection_id,
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: "existing-model".to_owned(),
            display_name: Some("Existing model".to_owned()),
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::UserOverride,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: now,
            last_seen_at: Some(now),
        };
        storage
            .save_model_route(&route)
            .expect("save modern direct route");
        storage
            .save_generation_preset(&GenerationPreset {
                id: GenerationPresetId::from("model-sync-modern-preset"),
                model_route_id: route.id.clone(),
                display_name: "Default".to_owned(),
                values: Vec::new(),
                reasoning: GenerationReasoningSettings::default(),
                prompt_cache: GenerationPromptCacheSettings::default(),
                created_at: now,
                updated_at: now,
            })
            .expect("save modern direct preset");
        assert_eq!(
            storage
                .get_provider_profile(connection.id.as_str())
                .expect_err("modern direct fixture must not retain a legacy profile")
                .code,
            lorepia_domain::CoreErrorCode::NotFound
        );
        (root, storage, connection, route)
    }

    fn archive_credential_bound_connection(
        storage: &Storage,
        connection_id: &ProviderConnectionId,
    ) {
        let archive = storage
            .prepare_provider_credential_operation(
                connection_id,
                ProviderCredentialOperationKind::RemoveForArchive,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("prepare terminal model-sync provider archive");
        storage
            .finish_provider_credential_archive(
                &archive.plan.operation_id,
                &archive.plan_sha256,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("terminal model-sync history permits provider archive");
    }

    fn install_credential_access_authority(
        storage: &Storage,
        connection_id: &ProviderConnectionId,
    ) -> crate::ProviderCredentialAccessAuthority {
        let authority = storage
            .propose_provider_credential_install_authority(connection_id)
            .expect("propose model-sync credential install authority");
        let install = storage
            .prepare_provider_credential_operation_with_install_authority(
                connection_id,
                ProviderCredentialOperationKind::Install,
                ProviderCredentialObservedStatus::Missing,
                Some(&authority),
            )
            .expect("prepare model-sync credential install");
        storage
            .start_provider_credential_operation(&install.plan.operation_id, &install.plan_sha256)
            .expect("start model-sync credential install");
        storage
            .finish_provider_credential_operation(
                &install.plan.operation_id,
                &install.plan_sha256,
                ProviderCredentialObservedStatus::Available,
            )
            .expect("finish model-sync credential install");
        storage
            .ensure_provider_credential_access_settled(connection_id)
            .expect("read installed model-sync credential authority")
    }

    fn terminally_remove_credential(storage: &Storage, connection_id: &ProviderConnectionId) {
        let removal = storage
            .prepare_provider_credential_operation(
                connection_id,
                ProviderCredentialOperationKind::RemoveCredential,
                ProviderCredentialObservedStatus::Available,
            )
            .expect("prepare model-sync credential removal");
        storage
            .start_provider_credential_operation(&removal.plan.operation_id, &removal.plan_sha256)
            .expect("start model-sync credential removal");
        storage
            .finish_provider_credential_operation(
                &removal.plan.operation_id,
                &removal.plan_sha256,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("finish model-sync credential removal");
    }

    fn assert_model_sync_blocks_credential_archive(
        storage: &Storage,
        connection_id: &ProviderConnectionId,
    ) {
        let error = storage
            .prepare_provider_credential_operation(
                connection_id,
                ProviderCredentialOperationKind::RemoveForArchive,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect_err("nonterminal model sync must block archive");
        assert_eq!(error.code, lorepia_domain::CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert_eq!(
            error.message,
            "provider connection cannot be archived while model synchronization is unfinished"
        );
    }

    fn normalized_route_metadata(capabilities: serde_json::Value) -> BoundedJson {
        BoundedJson::from_value(&json!({
            "capabilities": capabilities,
            "max_input_tokens": 128_000,
            "max_output_tokens": 16_384,
            "supported_generation_methods": [],
        }))
        .expect("bounded route metadata")
    }

    #[test]
    fn provider_api_metadata_semantics_reject_forged_exact_capability_claims() {
        let contradictions = [
            json!({
                "supported": ["reasoning"],
                "parameters": {"kind": "exact", "values": ["include_reasoning"]},
                "reasoning": null,
            }),
            json!({
                "supported": ["parallel_tool_calling"],
                "parameters": {"kind": "exact", "values": ["parallel_tool_calls"]},
                "reasoning": null,
            }),
            json!({
                "supported": [],
                "parameters": {"kind": "exact", "values": ["tools"]},
                "reasoning": null,
            }),
            json!({
                "supported": [],
                "parameters": {"kind": "exact", "values": ["temperature"]},
                "reasoning": {
                    "supported_efforts": {"kind": "not_exposed"},
                    "default_effort": null,
                    "default_enabled": null,
                    "supports_max_tokens": null,
                    "mandatory": null,
                },
            }),
        ];
        for capabilities in contradictions {
            let metadata = normalized_route_metadata(capabilities);
            validate_provider_api_route_metadata(Some(&metadata))
                .expect_err("normalized-looking semantic contradiction must fail closed");
        }
    }

    #[test]
    fn provider_api_metadata_semantics_accept_exact_and_backward_two_key_shapes() {
        let exact = normalized_route_metadata(json!({
            "supported": [
                "reasoning",
                "tool_calling",
                "parallel_tool_calling",
                "json_mode",
                "seed",
            ],
            "parameters": {
                "kind": "exact",
                "values": [
                    "include_reasoning",
                    "parallel_tool_calls",
                    "reasoning",
                    "response_format",
                    "seed",
                    "tools",
                ],
            },
            "reasoning": {
                "supported_efforts": {"kind": "exact", "values": ["high", "low"]},
                "default_effort": "high",
                "default_enabled": true,
                "supports_max_tokens": false,
                "mandatory": false,
            },
        }));
        validate_provider_api_route_metadata(Some(&exact))
            .expect("exact parameters derive the same supported capabilities");

        let backward = normalized_route_metadata(json!({
            "supported": [],
            "reasoning": null,
        }));
        validate_provider_api_route_metadata(Some(&backward))
            .expect("old normalized two-key capability metadata remains readable");
    }

    #[test]
    fn schema_eight_migrates_to_nine_and_backfills_existing_missing_routes() {
        let root = tempdir().expect("temporary data root");
        std::fs::create_dir_all(root.path().join("db")).expect("create database directory");
        let path = root.path().join("db/lorepia.sqlite3");
        let connection = rusqlite::Connection::open(path).expect("open version-eight database");
        for migration in [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_import_asset_recovery.sql"),
            include_str!("../migrations/0003_conversation_branches.sql"),
            include_str!("../migrations/0004_provider_catalog.sql"),
            include_str!("../migrations/0005_discovery_state_machine.sql"),
            include_str!("../migrations/0006_generation_provider_provenance.sql"),
            include_str!("../migrations/0007_signed_catalog_history.sql"),
            include_str!("../migrations/0008_generation_protocol_state.sql"),
        ] {
            connection
                .execute_batch(migration)
                .expect("apply historical migration");
        }
        for version in 1..=8 {
            connection
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at)
                     VALUES (?1, '2026-07-31T00:00:00Z')",
                    [version],
                )
                .expect("record historical migration");
        }
        connection
            .execute_batch(
                "INSERT INTO provider_templates
                 (id, version, display_name, source_kind, manifest_json,
                  manifest_sha256, created_at)
                 VALUES (
                   'migration-template', 1, 'Migration', 'built_in', '{}',
                   '0000000000000000000000000000000000000000000000000000000000000000',
                   '2026-07-31T00:00:00Z'
                 );
                 INSERT INTO provider_connections
                 (id, template_id, template_version, display_name, api_origin,
                  config_json, credential_ref, credential_scope_json,
                  timeout_seconds, status, created_at, updated_at)
                 VALUES (
                   'migration-connection', 'migration-template', 1, 'Migration',
                   'https://api.example.test', '{}', NULL, NULL, 30, 'connected',
                   '2026-07-31T00:00:00Z', '2026-07-31T00:00:00Z'
                 );
                 INSERT INTO provider_models
                 (id, connection_id, api_family, model_id, display_name, route_json,
                  availability, raw_metadata_json, first_seen_at, last_seen_at)
                 VALUES (
                   'migration-route', 'migration-connection',
                   'openai_chat_completions', 'missing-model', NULL, '{}',
                   'missing_temporarily', NULL,
                   '2026-07-31T00:00:00Z', '2026-07-31T00:00:00Z'
                 );",
            )
            .expect("seed version-eight provider graph");
        drop(connection);

        let storage = Storage::open(root.path()).expect("migrate storage to current schema");
        assert_eq!(
            storage
                .schema_version()
                .expect("read durable schema version"),
            crate::database::SCHEMA_VERSION
        );
        let route = storage
            .get_model_route(&ModelRouteId::from("migration-route"))
            .expect("migrated route");
        assert_eq!(route.miss_count, 1);
        assert_eq!(route.metadata_source, ModelMetadataSource::Legacy);
        assert!(route.last_reconciled_sync_job_id.is_none());
        assert!(route.metadata_sync_job_id.is_none());
    }

    fn review(
        job_id: &ModelSyncJobId,
        connection: &ProviderConnection,
        expected_routes: Vec<ModelRoute>,
        mut listed_routes: Vec<ModelRoute>,
        missing: Vec<ModelRouteId>,
        initial_presets: Vec<GenerationPreset>,
    ) -> ModelSyncReview {
        let observed_at = Utc::now();
        let expected_ids = expected_routes
            .iter()
            .map(|route| route.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for route in &mut listed_routes {
            if !expected_ids.contains(&route.id) {
                route.first_seen_at = observed_at;
            }
            route.last_reconciled_sync_job_id = Some(job_id.clone());
            route.metadata_sync_job_id = Some(job_id.clone());
            route.miss_count = 0;
            route.metadata_source = ModelMetadataSource::ProviderApi;
            route.metadata_observed_at = Some(observed_at);
            route.last_seen_at = Some(observed_at);
            route.raw_metadata = Some(
                lorepia_domain::BoundedJson::from_value(&serde_json::json!({
                    "capabilities": {
                        "supported": [],
                        "parameters": {"kind": "not_exposed"},
                        "reasoning": null,
                    },
                    "max_input_tokens": null,
                    "max_output_tokens": null,
                    "supported_generation_methods": [],
                }))
                .expect("normalized route metadata"),
            );
        }
        let newly_seen_model_route_ids = listed_routes
            .iter()
            .filter(|route| !expected_ids.contains(&route.id))
            .map(|route| route.id.clone())
            .collect::<Vec<_>>();
        let configured_routes = initial_presets
            .iter()
            .map(|preset| preset.model_route_id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let routes_requiring_preset_configuration = newly_seen_model_route_ids
            .iter()
            .filter(|id| !configured_routes.contains(*id))
            .cloned()
            .collect();
        ModelSyncReview::new(ModelSyncDiff {
            connection_id: connection.id.clone(),
            expected_connection: connection.clone(),
            expected_model_routes: expected_routes,
            observed_at,
            listed_routes,
            newly_seen_model_route_ids,
            missing_model_route_ids: missing,
            initial_presets,
            capability_observations: Vec::new(),
            routes_requiring_preset_configuration,
            provenance: ModelSyncSourceProvenance {
                source: "provider_api".to_owned(),
                api_family: ApiFamily::OpenAiChatCompletions,
                api_origin: connection.api_origin.clone(),
                endpoint_path: EndpointPath::parse("/v1/models").expect("models endpoint"),
                pages_fetched: 1,
                response_bytes: 128,
            },
        })
        .expect("canonical review")
    }

    #[test]
    fn active_job_is_unique_and_cancel_has_ordered_durable_events() {
        let (_root, storage, connection, _route) = seeded_storage();
        let created = storage
            .create_model_sync_job(&connection)
            .expect("create model sync");
        let duplicate = storage
            .create_model_sync_job(&connection)
            .expect_err("only one active job");
        assert_eq!(duplicate.code, lorepia_domain::CoreErrorCode::InvalidInput);
        let fetching = storage
            .transition_model_sync_job_to_fetching(&created.id, created.revision)
            .expect("start fetching");
        let cancelled = storage
            .cancel_model_sync_job(&fetching.id)
            .expect("cancel model sync");
        assert_eq!(cancelled.state, ModelSyncState::Cancelled);

        let events = storage
            .poll_model_sync_events_for_job(&created.id, 32)
            .expect("poll model sync events");
        assert_eq!(
            events.iter().map(|event| event.state).collect::<Vec<_>>(),
            vec![
                ModelSyncState::Created,
                ModelSyncState::Fetching,
                ModelSyncState::Cancelled,
            ]
        );
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence
                    && pair[0].job_revision < pair[1].job_revision)
        );
        assert_eq!(
            storage
                .poll_model_sync_events_for_job(&created.id, 32)
                .expect("unacknowledged events are delivered at least once"),
            events
        );
        for event in &events {
            assert!(
                storage
                    .ack_model_sync_event(&created.id, event.sequence)
                    .expect("ack model sync event")
            );
        }
        assert!(
            storage
                .poll_model_sync_events_for_job(&created.id, 32)
                .expect("job outbox is acknowledged")
                .is_empty()
        );
        let restarted = storage
            .create_model_sync_job(&connection)
            .expect("cancel releases active-job slot");
        let discovered = storage
            .list_model_sync_jobs(&connection.id, 8)
            .expect("rediscover jobs after restart");
        assert_eq!(discovered.len(), 2);
        assert_eq!(discovered[0].id, restarted.id);
        assert_eq!(discovered[1].id, cancelled.id);
    }

    #[test]
    fn prepared_credential_archive_blocks_new_model_sync_job() {
        let (_root, storage, connection, _route) = seeded_storage();
        let archive = storage
            .prepare_provider_credential_operation(
                &connection.id,
                ProviderCredentialOperationKind::RemoveForArchive,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("prepare credential archive before model sync");

        let error = storage
            .create_model_sync_job(&connection)
            .expect_err("prepared credential archive must reserve the connection");
        assert_eq!(error.code, lorepia_domain::CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert_eq!(
            error.message,
            "provider connection cannot start model synchronization while its credential operation is unresolved"
        );
        assert!(
            storage
                .list_model_sync_jobs(&connection.id, 4)
                .expect("list model sync jobs after rejected create")
                .is_empty()
        );
        assert_eq!(
            storage
                .list_unresolved_provider_credential_operations()
                .expect("list unresolved credential operations")
                .iter()
                .map(|operation| operation.plan.operation_id.as_str())
                .collect::<Vec<_>>(),
            vec![archive.plan.operation_id.as_str()]
        );
    }

    #[test]
    fn prepared_credential_removal_blocks_new_model_sync_job() {
        let (_root, storage, connection, _route) = seeded_storage();
        let removal = storage
            .prepare_provider_credential_operation(
                &connection.id,
                ProviderCredentialOperationKind::RemoveCredential,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("prepare credential removal before model sync");

        let error = storage
            .create_model_sync_job(&connection)
            .expect_err("prepared credential removal must reserve the connection");
        assert_eq!(error.code, lorepia_domain::CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert!(
            storage
                .list_model_sync_jobs(&connection.id, 4)
                .expect("list model sync jobs after rejected create")
                .is_empty()
        );
        assert_eq!(
            storage
                .list_unresolved_provider_credential_operations()
                .expect("list unresolved credential operations")
                .iter()
                .map(|operation| operation.plan.operation_id.as_str())
                .collect::<Vec<_>>(),
            vec![removal.plan.operation_id.as_str()]
        );
    }

    #[test]
    fn begun_model_sync_blocks_credential_removal_prepare() {
        let (_root, storage, connection, _route) = seeded_storage();
        let created = storage
            .create_model_sync_job(&connection)
            .expect("create model sync before credential removal");

        let error = storage
            .prepare_provider_credential_operation(
                &connection.id,
                ProviderCredentialOperationKind::RemoveCredential,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect_err("nonterminal model sync must block credential removal");
        assert_eq!(error.code, lorepia_domain::CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert_eq!(
            storage
                .get_model_sync_job(&created.id)
                .expect("model sync remains durable after rejected removal")
                .state,
            ModelSyncState::Created
        );
        assert!(
            storage
                .list_unresolved_provider_credential_operations()
                .expect("list unresolved credential operations")
                .is_empty()
        );
    }

    #[test]
    fn terminal_credential_removal_rejects_cached_model_sync_authority_without_a_job() {
        let (_root, storage, connection, _route) = seeded_storage();
        let cached_authority = install_credential_access_authority(&storage, &connection.id);
        terminally_remove_credential(&storage, &connection.id);

        let error = storage
            .create_model_sync_job_with_credential_access_authority(
                &connection,
                Some(&cached_authority),
            )
            .expect_err("removed credential authority must not start model sync");
        assert_eq!(error.code, lorepia_domain::CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert!(
            storage
                .list_model_sync_jobs(&connection.id, 4)
                .expect("list rejected model-sync jobs")
                .is_empty(),
            "authority CAS failure must happen before durable provider work"
        );
    }

    #[test]
    fn unfinished_model_sync_blocks_provider_archive_until_cancelled() {
        let (_root, storage, connection, _route) = seeded_storage();
        let created = storage
            .create_model_sync_job(&connection)
            .expect("create model sync");
        storage
            .save_settings(&AppSettings {
                selected_provider_profile_id: Some(connection.id.as_str().to_owned()),
                ..AppSettings::default()
            })
            .expect("select provider");

        assert_model_sync_blocks_credential_archive(&storage, &connection.id);
        assert_eq!(
            storage
                .get_provider_connection(&connection.id)
                .expect("provider remains active after rejected archive"),
            connection
        );
        assert_eq!(
            storage
                .load_settings()
                .expect("selection after rejected archive")
                .selected_provider_profile_id
                .as_deref(),
            Some("model-sync")
        );

        let cancelled = storage
            .cancel_model_sync_job(&created.id)
            .expect("cancel model sync");
        assert_eq!(cancelled.state, ModelSyncState::Cancelled);
        archive_credential_bound_connection(&storage, &connection.id);
        assert_eq!(
            storage
                .get_provider_connection(&connection.id)
                .expect_err("archived provider is hidden")
                .code,
            lorepia_domain::CoreErrorCode::NotFound
        );
        assert_eq!(
            storage
                .load_settings()
                .expect("selection after archive")
                .selected_provider_profile_id,
            None
        );
        assert_eq!(
            storage
                .list_model_sync_jobs(&connection.id, 4)
                .expect("terminal model sync history remains readable"),
            vec![cancelled]
        );
    }

    #[test]
    fn every_current_model_sync_state_obeys_archive_terminal_boundary() {
        let root = tempdir().expect("temporary data root");
        let storage = Storage::open(root.path()).expect("open storage");
        let states = [
            (ModelSyncState::Created, "created"),
            (ModelSyncState::Fetching, "fetching"),
            (ModelSyncState::Interrupted, "interrupted"),
            (
                ModelSyncState::DiffReadyAwaitingReview,
                "diff-ready-awaiting-review",
            ),
            (ModelSyncState::Committing, "committing"),
            (ModelSyncState::Completed, "completed"),
            (ModelSyncState::Failed, "failed"),
            (ModelSyncState::Cancelled, "cancelled"),
        ];

        for (state, state_label) in states {
            let profile_id = format!("archive-model-sync-{state_label}");
            storage
                .save_provider_profile(&ProviderProfile {
                    id: profile_id.clone(),
                    display_name: format!("Archive boundary {state_label}"),
                    base_url: "https://api.example.test/v1".to_owned(),
                    model: "boundary-model".to_owned(),
                    timeout_seconds: 30,
                })
                .expect("seed provider graph");
            let connection_id = ProviderConnectionId::from(profile_id.as_str());
            let connection = storage
                .get_provider_connection(&connection_id)
                .expect("seeded connection");
            let job = storage
                .create_model_sync_job(&connection)
                .expect("create model sync");
            let review_sha256 = "0".repeat(64);
            let has_review = matches!(
                state,
                ModelSyncState::DiffReadyAwaitingReview
                    | ModelSyncState::Committing
                    | ModelSyncState::Completed
            );
            let is_approved = matches!(
                state,
                ModelSyncState::Committing | ModelSyncState::Completed
            );
            let failure_json = (state == ModelSyncState::Failed).then(|| {
                serde_json::json!({
                    "code": "synthetic_failure",
                    "message_key": "model_sync.failed",
                    "recoverable": true,
                })
                .to_string()
            });
            storage
                .connection()
                .expect("database connection")
                .execute(
                    "UPDATE model_sync_jobs
                     SET state = ?2,
                         review_json = ?3,
                         review_sha256 = ?4,
                         approved_review_sha256 = ?5,
                         approved_at = ?6,
                         failure_json = ?7
                     WHERE id = ?1",
                    rusqlite::params![
                        job.id.as_str(),
                        state_label,
                        has_review.then_some("{}"),
                        has_review.then_some(review_sha256.as_str()),
                        is_approved.then_some(review_sha256.as_str()),
                        is_approved.then(|| Utc::now().to_rfc3339()),
                        failure_json,
                    ],
                )
                .expect("seed exact model-sync state");

            if state.is_terminal() {
                archive_credential_bound_connection(&storage, &connection_id);
                assert_eq!(
                    storage
                        .get_provider_connection(&connection_id)
                        .expect_err("terminal state permits hidden archive")
                        .code,
                    lorepia_domain::CoreErrorCode::NotFound
                );
            } else {
                assert_model_sync_blocks_credential_archive(&storage, &connection_id);
                assert!(
                    storage.get_provider_connection(&connection_id).is_ok(),
                    "rejected archive keeps provider active for {state_label}"
                );
            }
        }
    }

    #[test]
    fn job_scoped_event_poll_cannot_consume_another_jobs_events() {
        let (_root, storage, first_connection, _route) = seeded_storage();
        storage
            .save_provider_profile(&ProviderProfile {
                id: "model-sync-other".to_owned(),
                display_name: "Other model sync".to_owned(),
                base_url: "https://other-api.example.test/v1".to_owned(),
                model: "other-existing-model".to_owned(),
                timeout_seconds: 30,
            })
            .expect("seed second provider graph");
        let second_connection = storage
            .get_provider_connection(&ProviderConnectionId::from("model-sync-other"))
            .expect("second seeded connection");
        let first_job = storage
            .create_model_sync_job(&first_connection)
            .expect("create first job");
        let second_job = storage
            .create_model_sync_job(&second_connection)
            .expect("create second job");
        let first_fetching = storage
            .transition_model_sync_job_to_fetching(&first_job.id, first_job.revision)
            .expect("advance only the first job");
        assert_eq!(first_fetching.state, ModelSyncState::Fetching);
        assert_eq!(
            storage
                .get_model_sync_job(&second_job.id)
                .expect("second job remains isolated")
                .state,
            ModelSyncState::Created
        );

        let first_events = storage
            .poll_model_sync_events_for_job(&first_job.id, 32)
            .expect("poll first job");
        assert_eq!(first_events.len(), 2);
        assert!(
            first_events
                .iter()
                .all(|event| event.job_id == first_job.id)
        );
        assert!(
            !storage
                .ack_model_sync_event(&second_job.id, first_events[1].sequence)
                .expect("cross-job sequence must not acknowledge an event")
        );
        assert!(
            storage
                .ack_model_sync_event(&first_job.id, first_events[0].sequence)
                .expect("ack first job")
        );
        assert!(
            !storage
                .ack_model_sync_event(&first_job.id, first_events[0].sequence)
                .expect("duplicate first-job ack is idempotent")
        );

        let second_events = storage
            .poll_model_sync_events_for_job(&second_job.id, 32)
            .expect("poll second job after first was acknowledged");
        assert_eq!(second_events.len(), 1);
        assert_eq!(second_events[0].job_id, second_job.id);
        assert_eq!(second_events[0].sequence, first_events[0].sequence);
        assert_eq!(
            storage
                .poll_model_sync_events_for_job(&second_job.id, 32)
                .expect("second job remains available until its own ack"),
            second_events
        );
        assert!(
            storage
                .ack_model_sync_event(&second_job.id, second_events[0].sequence)
                .expect("ack second job")
        );
        let first_remaining = storage
            .poll_model_sync_events_for_job(&first_job.id, 32)
            .expect("first job's later event remains isolated");
        assert_eq!(first_remaining, vec![first_events[1].clone()]);
    }

    #[test]
    fn reopen_interrupts_fetching_without_replaying_network_work() {
        let (root, storage, connection, _route) = seeded_storage();
        let created = storage
            .create_model_sync_job(&connection)
            .expect("create model sync");
        storage
            .transition_model_sync_job_to_fetching(&created.id, created.revision)
            .expect("start fetching");
        drop(storage);

        let reopened = Storage::open(root.path()).expect("reopen storage");
        let recovered = reopened
            .get_model_sync_job(&created.id)
            .expect("recovered job");
        assert_eq!(recovered.state, ModelSyncState::Interrupted);
        reopened
            .cancel_model_sync_job(&created.id)
            .expect("interrupted job can be cancelled");
    }

    #[test]
    fn review_ready_job_survives_reopen_and_is_discoverable_and_cancellable() {
        let (root, storage, connection, route) = seeded_storage();
        let created = storage
            .create_model_sync_job(&connection)
            .expect("create model sync");
        let fetching = storage
            .transition_model_sync_job_to_fetching(&created.id, created.revision)
            .expect("start fetching");
        let staged = review(
            &created.id,
            &connection,
            vec![route.clone()],
            Vec::new(),
            vec![route.id],
            Vec::new(),
        );
        storage
            .store_model_sync_review(&created.id, fetching.revision, &staged)
            .expect("store review");
        drop(storage);

        let reopened = Storage::open(root.path()).expect("reopen review-ready storage");
        let discovered = reopened
            .list_model_sync_jobs(&connection.id, 4)
            .expect("discover review-ready job");
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].state, ModelSyncState::DiffReadyAwaitingReview);
        assert_eq!(
            discovered[0]
                .review
                .as_ref()
                .expect("durable review")
                .sha256,
            staged.sha256
        );
        assert_eq!(
            reopened
                .cancel_model_sync_job(&created.id)
                .expect("cancel review-ready job")
                .state,
            ModelSyncState::Cancelled
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn approval_recomputes_hash_and_repeated_omission_counts_once_per_job() {
        let (_root, storage, connection, route) = seeded_modern_storage();
        let created = storage
            .create_model_sync_job(&connection)
            .expect("create first model sync");
        let fetching = storage
            .transition_model_sync_job_to_fetching(&created.id, created.revision)
            .expect("start fetching");
        let first_review = review(
            &created.id,
            &connection,
            vec![route.clone()],
            Vec::new(),
            vec![route.id.clone()],
            Vec::new(),
        );
        let awaiting = storage
            .store_model_sync_review(&created.id, fetching.revision, &first_review)
            .expect("store review");
        storage
            .mark_model_sync_job_committing(&created.id, awaiting.revision, "00")
            .expect_err("wrong approval hash is rejected");
        assert_eq!(
            storage
                .get_model_route(&route.id)
                .expect("unchanged route")
                .miss_count,
            0
        );
        let committing = storage
            .mark_model_sync_job_committing(&created.id, awaiting.revision, &first_review.sha256)
            .expect("approve exact review");
        let completed = storage
            .commit_model_sync_job(&created.id, committing.revision)
            .expect("commit first omission");
        assert_eq!(completed.state, ModelSyncState::Completed);
        assert_eq!(
            storage
                .commit_model_sync_job(&created.id, committing.revision)
                .expect("replayed commit returns stored result")
                .state,
            ModelSyncState::Completed
        );
        let first_missing = storage
            .get_model_route(&route.id)
            .expect("first missing route");
        assert_eq!(first_missing.miss_count, 1);
        assert_eq!(first_missing.status, ModelAvailability::MissingTemporarily);
        assert_eq!(
            first_missing.last_reconciled_sync_job_id,
            Some(created.id.clone())
        );
        assert_eq!(
            storage
                .list_generation_presets(&route.id)
                .expect("presets remain")
                .len(),
            1
        );

        let second = storage
            .create_model_sync_job(
                &storage
                    .get_provider_connection(&connection.id)
                    .expect("updated connection"),
            )
            .expect("create second model sync");
        let second_fetching = storage
            .transition_model_sync_job_to_fetching(&second.id, second.revision)
            .expect("start second fetch");
        let current_connection = storage
            .get_provider_connection(&connection.id)
            .expect("current connection");
        let second_review = review(
            &second.id,
            &current_connection,
            vec![first_missing],
            Vec::new(),
            vec![route.id.clone()],
            Vec::new(),
        );
        let second_awaiting = storage
            .store_model_sync_review(&second.id, second_fetching.revision, &second_review)
            .expect("store second review");
        let second_committing = storage
            .mark_model_sync_job_committing(
                &second.id,
                second_awaiting.revision,
                &second_review.sha256,
            )
            .expect("approve second review");
        storage
            .commit_model_sync_job(&second.id, second_committing.revision)
            .expect("commit second omission");
        let twice_missing = storage
            .get_model_route(&route.id)
            .expect("twice missing route");
        assert_eq!(twice_missing.miss_count, 2);

        let current_connection = storage
            .get_provider_connection(&connection.id)
            .expect("connection before reappearance");
        let reappearance = storage
            .create_model_sync_job(&current_connection)
            .expect("create reappearance job");
        let reappearance_fetching = storage
            .transition_model_sync_job_to_fetching(&reappearance.id, reappearance.revision)
            .expect("start reappearance fetch");
        let mut seen_again = twice_missing.clone();
        seen_again.status = ModelAvailability::Available;
        seen_again.metadata_source = ModelMetadataSource::ProviderApi;
        seen_again.metadata_observed_at = Some(Utc::now());
        let reappearance_review = review(
            &reappearance.id,
            &current_connection,
            vec![twice_missing],
            vec![seen_again],
            Vec::new(),
            Vec::new(),
        );
        let reappearance_awaiting = storage
            .store_model_sync_review(
                &reappearance.id,
                reappearance_fetching.revision,
                &reappearance_review,
            )
            .expect("store reappearance review");
        let reappearance_committing = storage
            .mark_model_sync_job_committing(
                &reappearance.id,
                reappearance_awaiting.revision,
                &reappearance_review.sha256,
            )
            .expect("approve reappearance");
        storage
            .commit_model_sync_job(&reappearance.id, reappearance_committing.revision)
            .expect("commit reappearance");
        let reappeared = storage
            .get_model_route(&route.id)
            .expect("reappeared route");
        assert_eq!(reappeared.miss_count, 0);
        assert_eq!(reappeared.status, ModelAvailability::Available);
        assert_eq!(
            reappeared.metadata_sync_job_id,
            Some(reappearance.id.clone())
        );

        let mut deprecated = reappeared.clone();
        deprecated.status = ModelAvailability::Deprecated;
        storage
            .save_model_route(&deprecated)
            .expect("explicitly deprecate route");
        let current_connection = storage
            .get_provider_connection(&connection.id)
            .expect("connection before deprecated omission");
        let deprecated_job = storage
            .create_model_sync_job(&current_connection)
            .expect("create deprecated omission job");
        let deprecated_fetching = storage
            .transition_model_sync_job_to_fetching(&deprecated_job.id, deprecated_job.revision)
            .expect("start deprecated omission fetch");
        let deprecated_review = review(
            &deprecated_job.id,
            &current_connection,
            vec![deprecated.clone()],
            Vec::new(),
            vec![deprecated.id.clone()],
            Vec::new(),
        );
        let deprecated_awaiting = storage
            .store_model_sync_review(
                &deprecated_job.id,
                deprecated_fetching.revision,
                &deprecated_review,
            )
            .expect("store deprecated omission review");
        let deprecated_committing = storage
            .mark_model_sync_job_committing(
                &deprecated_job.id,
                deprecated_awaiting.revision,
                &deprecated_review.sha256,
            )
            .expect("approve deprecated omission");
        storage
            .commit_model_sync_job(&deprecated_job.id, deprecated_committing.revision)
            .expect("commit deprecated omission");
        let still_deprecated = storage
            .get_model_route(&route.id)
            .expect("deprecated route remains");
        assert_eq!(still_deprecated.status, ModelAvailability::Deprecated);
        assert_eq!(still_deprecated.miss_count, 1);
        assert_eq!(
            still_deprecated.metadata_sync_job_id,
            reappeared.metadata_sync_job_id
        );
    }

    #[test]
    fn concurrent_graph_edit_rejects_commit_and_stable_identity_is_immutable() {
        let (_root, storage, connection, route) = seeded_modern_storage();
        let created = storage
            .create_model_sync_job(&connection)
            .expect("create model sync");
        let fetching = storage
            .transition_model_sync_job_to_fetching(&created.id, created.revision)
            .expect("start fetching");
        let staged_review = review(
            &created.id,
            &connection,
            vec![route.clone()],
            Vec::new(),
            vec![route.id.clone()],
            Vec::new(),
        );
        let awaiting = storage
            .store_model_sync_review(&created.id, fetching.revision, &staged_review)
            .expect("store review");

        let mut edited = route.clone();
        edited.display_name = Some("User edited name".to_owned());
        storage
            .save_model_route(&edited)
            .expect("concurrent user-visible edit");
        let committing = storage
            .mark_model_sync_job_committing(&created.id, awaiting.revision, &staged_review.sha256)
            .expect("approval CAS");
        storage
            .commit_model_sync_job(&created.id, committing.revision)
            .expect_err("graph hash rejects concurrent edit");
        let after = storage
            .get_model_route(&route.id)
            .expect("route after rejected commit");
        assert_eq!(after.display_name.as_deref(), Some("User edited name"));
        assert_eq!(after.miss_count, 0);

        let mut renamed_identity = after;
        renamed_identity.model_id = "silently-renamed-model".to_owned();
        storage
            .save_model_route(&renamed_identity)
            .expect_err("stable model identity cannot be renamed");
    }

    #[test]
    fn failed_apply_rolls_back_new_route_and_preset_together() {
        let (_root, storage, connection, existing) = seeded_storage();
        let created = storage
            .create_model_sync_job(&connection)
            .expect("create model sync");
        let fetching = storage
            .transition_model_sync_job_to_fetching(&created.id, created.revision)
            .expect("start fetching");
        let now = Utc::now();
        let new_route = ModelRoute {
            id: ModelRouteId::from("new-sync-route"),
            connection_id: connection.id.clone(),
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: "new-model".to_owned(),
            display_name: Some("New model".to_owned()),
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::ProviderApi,
            metadata_observed_at: Some(now),
            last_reconciled_sync_job_id: Some(created.id.clone()),
            metadata_sync_job_id: Some(created.id.clone()),
            first_seen_at: now,
            last_seen_at: Some(now),
        };
        // Reusing the legacy preset ID for a different route is invalid, but
        // validation happens after the new route write inside the transaction.
        let conflicting_preset = GenerationPreset {
            id: lorepia_domain::GenerationPresetId::from("model-sync"),
            model_route_id: new_route.id.clone(),
            display_name: "Conflicting".to_owned(),
            values: Vec::new(),
            reasoning: GenerationReasoningSettings::default(),
            prompt_cache: GenerationPromptCacheSettings::default(),
            created_at: now,
            updated_at: now,
        };
        let staged_review = review(
            &created.id,
            &connection,
            vec![existing.clone()],
            vec![new_route.clone()],
            vec![existing.id.clone()],
            vec![conflicting_preset],
        );
        let awaiting = storage
            .store_model_sync_review(&created.id, fetching.revision, &staged_review)
            .expect("store review");
        let committing = storage
            .mark_model_sync_job_committing(&created.id, awaiting.revision, &staged_review.sha256)
            .expect("approve review");
        storage
            .commit_model_sync_job(&created.id, committing.revision)
            .expect_err("conflicting preset aborts apply");
        assert!(
            storage.get_model_route(&new_route.id).is_err(),
            "route insertion must roll back with preset failure"
        );
        let unchanged = storage
            .get_model_route(&existing.id)
            .expect("existing route remains");
        assert_eq!(unchanged.miss_count, 0);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn approved_model_sync_replaces_provider_api_observation_snapshot_and_reopens() {
        let (root, storage, connection, route) = seeded_storage();
        let seed_time = Utc::now() - Duration::hours(1);
        let observation =
            |id: &str, key: CapabilityKey, value: CapabilityValue, source: ObservationSource| {
                CapabilityObservation {
                    id: ObservationId::from(id),
                    model_route_id: route.id.clone(),
                    key,
                    value,
                    status: SupportStatus::Verified,
                    source,
                    confidence: Confidence::High,
                    observed_at: seed_time,
                    expires_at: None,
                    evidence_ref: None,
                }
            };
        let old_max_output = observation(
            "model-sync:provider-api:max-output",
            CapabilityKey::MaxOutputTokens,
            CapabilityValue::Integer(4_096),
            ObservationSource::ProviderApi,
        );
        let legacy_prompt_caching = observation(
            "model-sync:provider-api:legacy-prompt-caching",
            CapabilityKey::PromptCaching,
            CapabilityValue::Boolean(true),
            ObservationSource::ProviderApi,
        );
        let signed_catalog = observation(
            "model-sync:signed:reasoning",
            CapabilityKey::Reasoning,
            CapabilityValue::Boolean(true),
            ObservationSource::SignedLorepiaCatalog,
        );
        storage
            .upsert_capability_observations(&[
                old_max_output.clone(),
                legacy_prompt_caching.clone(),
                signed_catalog.clone(),
            ])
            .expect("seed prior observations");

        let created = storage
            .create_model_sync_job(&connection)
            .expect("create first model sync");
        let fetching = storage
            .transition_model_sync_job_to_fetching(&created.id, created.revision)
            .expect("start first fetch");
        let mut first_diff = review(
            &created.id,
            &connection,
            vec![route.clone()],
            vec![route.clone()],
            Vec::new(),
            Vec::new(),
        )
        .diff;
        let current_context = CapabilityObservation {
            id: ObservationId::from("model-sync:provider-api:context-window"),
            model_route_id: route.id.clone(),
            key: CapabilityKey::ContextWindow,
            value: CapabilityValue::Integer(128_000),
            status: SupportStatus::Verified,
            source: ObservationSource::ProviderApi,
            confidence: Confidence::High,
            observed_at: first_diff.observed_at,
            expires_at: Some(first_diff.observed_at + Duration::hours(24)),
            evidence_ref: None,
        };
        first_diff.capability_observations = vec![current_context.clone()];
        let first_review = ModelSyncReview::new(first_diff).expect("canonical first review");
        let awaiting = storage
            .store_model_sync_review(&created.id, fetching.revision, &first_review)
            .expect("store first review");
        let committing = storage
            .mark_model_sync_job_committing(&created.id, awaiting.revision, &first_review.sha256)
            .expect("approve first review");
        storage
            .commit_model_sync_job(&created.id, committing.revision)
            .expect("commit first provider API snapshot");

        let after_first = storage
            .list_capability_observations(&route.id)
            .expect("observations after first snapshot");
        assert!(after_first.contains(&current_context));
        assert!(after_first.contains(&signed_catalog));
        assert!(!after_first.contains(&old_max_output));
        assert!(!after_first.contains(&legacy_prompt_caching));

        let current_connection = storage
            .get_provider_connection(&connection.id)
            .expect("connection after first sync");
        let current_route = storage
            .get_model_route(&route.id)
            .expect("route after first sync");
        let second = storage
            .create_model_sync_job(&current_connection)
            .expect("create second model sync");
        let second_fetching = storage
            .transition_model_sync_job_to_fetching(&second.id, second.revision)
            .expect("start second fetch");
        let second_review = review(
            &second.id,
            &current_connection,
            vec![current_route.clone()],
            vec![current_route],
            Vec::new(),
            Vec::new(),
        );
        let second_awaiting = storage
            .store_model_sync_review(&second.id, second_fetching.revision, &second_review)
            .expect("store second review");
        let second_committing = storage
            .mark_model_sync_job_committing(
                &second.id,
                second_awaiting.revision,
                &second_review.sha256,
            )
            .expect("approve second review");
        storage
            .connection()
            .expect("database")
            .execute_batch(
                "CREATE TEMP TRIGGER reject_model_sync_snapshot_publish
                 BEFORE UPDATE ON provider_connections
                 WHEN OLD.id = 'model-sync'
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic model sync publish failure');
                 END;",
            )
            .expect("install rollback trigger");
        storage
            .commit_model_sync_job(&second.id, second_committing.revision)
            .expect_err("post-delete publish failure must roll back the snapshot");
        let after_rollback = storage
            .list_capability_observations(&route.id)
            .expect("observations after rolled-back model sync");
        assert!(after_rollback.contains(&current_context));
        assert!(after_rollback.contains(&signed_catalog));
        storage
            .connection()
            .expect("database")
            .execute_batch("DROP TRIGGER reject_model_sync_snapshot_publish;")
            .expect("remove rollback trigger");
        storage
            .commit_model_sync_job(&second.id, second_committing.revision)
            .expect("commit empty provider API snapshot");
        assert_eq!(
            storage
                .list_capability_observations(&route.id)
                .expect("observations after omission"),
            vec![signed_catalog.clone()]
        );

        drop(storage);
        let reopened = Storage::open(root.path()).expect("reopen replaced snapshot");
        assert_eq!(
            reopened
                .list_capability_observations(&route.id)
                .expect("observations after reopen"),
            vec![signed_catalog]
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the rejection matrix keeps every durable review invariant in one fixture"
    )]
    fn model_sync_review_rejects_noncanonical_provider_api_observations() {
        let (_root, storage, connection, route) = seeded_storage();
        let prior = CapabilityObservation {
            id: ObservationId::from("model-sync:prior-provider-api"),
            model_route_id: route.id.clone(),
            key: CapabilityKey::ContextWindow,
            value: CapabilityValue::Integer(16_384),
            status: SupportStatus::Verified,
            source: ObservationSource::ProviderApi,
            confidence: Confidence::High,
            observed_at: Utc::now() - Duration::hours(1),
            expires_at: None,
            evidence_ref: None,
        };
        storage
            .upsert_capability_observation(&prior)
            .expect("seed prior observation");
        let created = storage
            .create_model_sync_job(&connection)
            .expect("create model sync");
        let fetching = storage
            .transition_model_sync_job_to_fetching(&created.id, created.revision)
            .expect("start fetch");
        let base_diff = review(
            &created.id,
            &connection,
            vec![route.clone()],
            vec![route.clone()],
            Vec::new(),
            Vec::new(),
        )
        .diff;
        let valid = CapabilityObservation {
            id: ObservationId::from("model-sync:canonical-provider-api"),
            model_route_id: route.id.clone(),
            key: CapabilityKey::ToolCalling,
            value: CapabilityValue::Boolean(true),
            status: SupportStatus::Verified,
            source: ObservationSource::ProviderApi,
            confidence: Confidence::High,
            observed_at: base_diff.observed_at,
            expires_at: Some(base_diff.observed_at + Duration::hours(24)),
            evidence_ref: None,
        };
        let mut invalid = Vec::new();
        invalid.push({
            let mut observation = valid.clone();
            observation.source = ObservationSource::SignedLorepiaCatalog;
            observation
        });
        invalid.push({
            let mut observation = valid.clone();
            observation.status = SupportStatus::Documented;
            observation
        });
        invalid.push({
            let mut observation = valid.clone();
            observation.confidence = Confidence::Medium;
            observation
        });
        invalid.push({
            let mut observation = valid.clone();
            observation.evidence_ref = Some(lorepia_domain::EvidenceId::from("foreign-evidence"));
            observation
        });
        invalid.push({
            let mut observation = valid.clone();
            observation.expires_at = None;
            observation
        });
        invalid.push({
            let mut observation = valid.clone();
            observation.key = CapabilityKey::PromptCaching;
            observation
        });
        invalid.push({
            let mut observation = valid.clone();
            observation.value = CapabilityValue::Boolean(false);
            observation
        });
        invalid.push({
            let mut observation = valid.clone();
            observation.status = SupportStatus::Unsupported;
            observation
        });
        for observation in invalid {
            let mut diff = base_diff.clone();
            diff.capability_observations = vec![observation];
            let invalid_review = ModelSyncReview::new(diff).expect("hash invalid review shape");
            storage
                .store_model_sync_review(&created.id, fetching.revision, &invalid_review)
                .expect_err("noncanonical provider API observation must be rejected");
            let unchanged = storage
                .get_model_sync_job(&created.id)
                .expect("unchanged model sync job");
            assert_eq!(unchanged.state, ModelSyncState::Fetching);
            assert_eq!(unchanged.revision, fetching.revision);
        }
        assert_eq!(
            storage
                .list_capability_observations(&route.id)
                .expect("prior observation remains"),
            vec![prior]
        );
    }
}
