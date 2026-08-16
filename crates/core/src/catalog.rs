//! Durable provider/model catalog import, history, diff, and rollback.
//!
//! Signed catalog files are selected locally by a native client. This module
//! verifies them against the trust roots compiled into `lorepia-providers`
//! before any bytes are committed. Credentials and pricing are deliberately
//! outside this catalog contract.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use lorepia_domain::{
    CapabilityKey, CapabilityObservation, CapabilityValue, Confidence, CoreError, CoreErrorCode,
    CoreResult, EvidenceId, ModelRoute, ObservationId, ObservationSource, ParameterSpec,
    ProviderTemplate, ProviderTemplateId, SupportStatus,
    discovery::DiscoveryCatalogAuthorityBinding,
};
use lorepia_providers::catalog::{
    BUNDLED_CATALOG_REVISION, BUNDLED_CATALOG_VERIFIED_AT, CATALOG_HISTORY_SCHEMA_VERSION,
    CatalogAuthority, CatalogCapabilityKey, CatalogCapabilityValue, CatalogDiffDto, CatalogError,
    CatalogErrorKind, CatalogFreshness, CatalogFreshnessPolicy, CatalogHistory,
    CatalogRevisionGuard, CatalogRevisionSnapshot, CatalogRollbackPlanDto, MergedCatalog,
    MergedCatalogModel, ModelMatch, SignedCatalogEnvelope, VerifiedCatalogUpdate,
    merge_with_bundled_catalog, verify_manual_catalog_import, verify_stored_catalog_update,
};
use lorepia_storage::{
    CatalogActivationKind, CatalogActivationRecord, CatalogImportCommit, CatalogRollbackCommit,
    CatalogSnapshotSource, CatalogStorageError, NewCatalogSnapshot, NewSignedCatalogUpdate,
    Storage, StoredCatalogRevisionGuard, StoredCatalogSnapshot, StoredCatalogState,
    StoredSignedCatalogUpdate,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::app::Core;

pub const PROVIDER_CATALOG_STATUS_SCHEMA_VERSION: u32 = 1;
pub const PROVIDER_CATALOG_HISTORY_SCHEMA_VERSION: u32 = 1;
pub const PROVIDER_CATALOG_IMPORT_PLAN_SCHEMA_VERSION: u32 = 1;
const MAX_CATALOG_HISTORY_PAGE_SIZE: u32 = 100;
const CATALOG_STORAGE_PAGE_SIZE: u32 = 200;
const CATALOG_IMPORT_PLAN_LIFETIME_MINUTES: i64 = 15;
const MAX_PENDING_CATALOG_IMPORT_PLANS: usize = 32;
const CATALOG_ROLLBACK_AUDIT_PLAN_VERSION: u32 = 2;

/// Secret-free summary of the currently active durable catalog.
///
/// `highest_accepted_revision` is the signed anti-rollback guard. It can be
/// higher than every entry in `active_signed_revisions` after a local history
/// rollback and is never lowered by activation changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogStatus {
    pub status_schema_version: u32,
    pub state_version: u64,
    pub active_revision: u64,
    pub active_snapshot_sha256: String,
    pub bundled_baseline_sha256: String,
    pub snapshot_count: u32,
    pub signed_update_count: u32,
    pub highest_accepted_revision: u64,
    pub latest_issued_at: Option<DateTime<Utc>>,
    pub active_signed_revisions: Vec<u64>,
}

/// One immutable merged snapshot in local history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogRevisionSummary {
    pub revision: u64,
    pub captured_at: DateTime<Utc>,
    pub snapshot_sha256: String,
    pub signed_revisions: Vec<u64>,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCatalogActivationKind {
    Import,
    Rollback,
}

/// Append-only record of a catalog pointer transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogActivationSummary {
    pub action_id: String,
    pub state_version: u64,
    pub kind: ProviderCatalogActivationKind,
    pub from_revision: Option<u64>,
    pub to_revision: u64,
    pub activated_at: DateTime<Utc>,
    pub diff: CatalogDiffDto,
}

/// Durable revision and activation history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogHistory {
    pub history_schema_version: u32,
    pub active_revision: u64,
    pub revisions: Vec<ProviderCatalogRevisionSummary>,
    pub activations: Vec<ProviderCatalogActivationSummary>,
    pub next_before_revision: Option<u64>,
    pub next_before_state_version: Option<u64>,
}

/// Result of accepting one signed update and activating its merged snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogImportResult {
    pub signed_catalog_revision: u64,
    pub activated_revision: u64,
    pub diff: CatalogDiffDto,
    pub status: ProviderCatalogStatus,
}

/// Secret-free, bounded review material for one exact signed catalog file.
///
/// Raw envelope bytes are intentionally excluded. Native clients retain the
/// selected file bytes and must submit those exact bytes with this plan during
/// activation. Every exposed field, including the typed diff, is committed by
/// `plan_sha256`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogImportReview {
    pub plan_schema_version: u32,
    pub action_id: String,
    pub expected_state_version: u64,
    pub expected_active_revision: u64,
    pub expected_active_snapshot_sha256: String,
    pub expected_highest_accepted_revision: u64,
    pub envelope_byte_count: u64,
    pub envelope_sha256: String,
    pub signing_key_id: String,
    pub payload_sha256: String,
    pub signed_catalog_revision: u64,
    pub candidate_revision: u64,
    pub candidate_snapshot_sha256: String,
    pub prepared_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub diff: CatalogDiffDto,
}

/// One short-lived import plan registered by this live Core instance.
///
/// The registry-side commitment makes mutation of the otherwise serializable
/// DTO detectable. Plans do not survive process restart and never contain
/// credentials or raw signed-envelope bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogImportPlan {
    pub review: ProviderCatalogImportReview,
    pub plan_sha256: String,
}

pub const PROVIDER_CATALOG_ROLLBACK_PLAN_SCHEMA_VERSION: u32 = 1;

/// One short-lived rollback review bound to the exact durable catalog state.
///
/// The catalog engine binds hashes and revisions. `expected_state_version`
/// additionally prevents replay after the active pointer moves away and later
/// returns to the same snapshot during the engine plan's validity window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogRollbackPlan {
    pub plan_schema_version: u32,
    pub action_id: String,
    pub expected_state_version: u64,
    pub plan_sha256: String,
    pub catalog_plan: CatalogRollbackPlanDto,
}

/// Result of activating a previously reviewed rollback plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogRollbackResult {
    pub from_revision: u64,
    pub activated_revision: u64,
    pub status: ProviderCatalogStatus,
}

#[derive(Serialize)]
struct CatalogRollbackAuditPlan<'a> {
    rollback_plan_version: u32,
    from_revision: u64,
    to_revision: u64,
    expected_active_sha256: &'a str,
    target_sha256: &'a str,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    diff_sha256: &'a str,
}

fn bundled_baseline_snapshot() -> CoreResult<CatalogRevisionSnapshot> {
    let captured_at = DateTime::parse_from_rfc3339(BUNDLED_CATALOG_VERIFIED_AT)
        .map_err(|_| CoreError::internal("bundled catalog timestamp is invalid"))?
        .with_timezone(&Utc);
    let merged =
        merge_with_bundled_catalog(&[], &[], captured_at, &CatalogFreshnessPolicy::default())
            .map_err(catalog_internal_error)?;
    CatalogRevisionSnapshot::from_merged(1, captured_at, &merged).map_err(catalog_internal_error)
}

fn catalog_request_error(error: CatalogError) -> CoreError {
    let code = match error.kind() {
        CatalogErrorKind::RollbackTargetMissing => CoreErrorCode::NotFound,
        _ => CoreErrorCode::InvalidInput,
    };
    CoreError::new(code, error.to_string(), false)
}

fn catalog_internal_error(error: CatalogError) -> CoreError {
    CoreError::new(
        CoreErrorCode::Internal,
        format!("catalog engine invariant failed: {error}"),
        false,
    )
}

fn catalog_storage_error(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn serialized_sha256(value: &impl Serialize) -> CoreResult<String> {
    serde_json::to_vec(value)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|_| CoreError::internal("catalog review plan could not be serialized"))
}

#[derive(Debug, Clone)]
struct ActiveCatalog {
    stored: StoredCatalogSnapshot,
    snapshot: CatalogRevisionSnapshot,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingProviderCatalogImportPlan {
    plan_sha256: String,
    envelope_sha256: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct CatalogImportCandidate {
    state: StoredCatalogState,
    active: ActiveCatalog,
    verified: VerifiedCatalogUpdate,
    canonical_payload: Vec<u8>,
    envelope_version: u32,
    envelope_sha256: String,
    snapshot: CatalogRevisionSnapshot,
    snapshot_sha256: String,
    diff: CatalogDiffDto,
}

/// One state-consistent, freshness-evaluated view of the active catalog.
///
/// Callers keep this value for the duration of one operation. A concurrent
/// import or rollback therefore affects the next operation without mixing two
/// active revisions inside the current one.
#[derive(Debug, Clone)]
pub(crate) struct OperationalCatalogProjection {
    pub(crate) state_version: u64,
    pub(crate) snapshot: CatalogRevisionSnapshot,
    merged: MergedCatalog,
    signed_layer_expirations: BTreeMap<String, DateTime<Utc>>,
}

/// Model-specific catalog material resolved for exactly one durable route.
#[derive(Debug, Clone, Default)]
pub(crate) struct CatalogRouteProjection {
    pub(crate) matched: bool,
    pub(crate) parameters: Vec<ParameterSpec>,
    /// Fresh parameter contracts whose selected field provenance is an
    /// activated, signature-verified catalog layer rather than the bundled
    /// family baseline.
    pub(crate) signed_parameters: Vec<ParameterSpec>,
    pub(crate) capability_observations: Vec<CapabilityObservation>,
}

impl Core {
    /// Verify an exact signed file and register a short-lived review plan.
    ///
    /// This method never activates or persists catalog content. The returned
    /// plan is secret-free and contains a typed diff; the native caller keeps
    /// the bounded input bytes until explicit activation.
    pub fn prepare_signed_provider_catalog_import(
        &self,
        envelope_json: &[u8],
    ) -> CoreResult<ProviderCatalogImportPlan> {
        let now = Utc::now();
        let candidate = build_catalog_import_candidate(self, envelope_json, now, now)?;
        let action_id = format!("catalog-import-{}", Uuid::new_v4());
        let payload_expires_at = candidate.verified.payload().expires_at;
        let review = ProviderCatalogImportReview {
            plan_schema_version: PROVIDER_CATALOG_IMPORT_PLAN_SCHEMA_VERSION,
            action_id: action_id.clone(),
            expected_state_version: candidate.state.state_version,
            expected_active_revision: candidate.active.snapshot.revision,
            expected_active_snapshot_sha256: candidate.active.stored.snapshot_sha256.clone(),
            expected_highest_accepted_revision: candidate.state.guard.highest_accepted_revision,
            envelope_byte_count: u64::try_from(envelope_json.len())
                .map_err(|_| CoreError::invalid("catalog envelope is too large"))?,
            envelope_sha256: candidate.envelope_sha256.clone(),
            signing_key_id: candidate.verified.signing_key_id().to_owned(),
            payload_sha256: candidate.verified.payload_sha256().to_owned(),
            signed_catalog_revision: candidate.verified.payload().revision,
            candidate_revision: candidate.snapshot.revision,
            candidate_snapshot_sha256: candidate.snapshot_sha256,
            prepared_at: now,
            expires_at: (now + Duration::minutes(CATALOG_IMPORT_PLAN_LIFETIME_MINUTES))
                .min(payload_expires_at),
            diff: candidate.diff,
        };
        let plan_sha256 = serialized_sha256(&review)?;
        let plan = ProviderCatalogImportPlan {
            review,
            plan_sha256: plan_sha256.clone(),
        };
        register_pending_catalog_import_plan(self, &plan, now)?;
        Ok(plan)
    }

    /// Activate exactly one unmodified, live import review.
    ///
    /// Signature verification, merge, immutable-version validation, typed diff,
    /// and every state/hash binding are recomputed before the durable atomic
    /// commit. A successful plan is consumed and cannot be replayed.
    pub fn activate_signed_provider_catalog_import(
        &self,
        plan: &ProviderCatalogImportPlan,
        envelope_json: &[u8],
    ) -> CoreResult<ProviderCatalogImportResult> {
        let now = Utc::now();
        let recomputed_plan_sha256 = serialized_sha256(&plan.review)?;
        let mut pending = self
            .pending_catalog_import_plans()
            .lock()
            .map_err(|_| CoreError::internal("catalog import review lock was poisoned"))?;
        let Some(registered) = pending.get(&plan.review.action_id).cloned() else {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "catalog import plan is unknown or was already used",
                false,
            ));
        };
        if registered.expires_at <= now || plan.review.expires_at <= now {
            pending.remove(&plan.review.action_id);
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "catalog import plan has expired",
                true,
            ));
        }
        let envelope_sha256 = sha256_hex(envelope_json);
        if plan.review.plan_schema_version != PROVIDER_CATALOG_IMPORT_PLAN_SCHEMA_VERSION
            || recomputed_plan_sha256 != plan.plan_sha256
            || registered.plan_sha256 != plan.plan_sha256
            || registered.envelope_sha256 != envelope_sha256
            || plan.review.envelope_sha256 != envelope_sha256
            || plan.review.envelope_byte_count
                != u64::try_from(envelope_json.len())
                    .map_err(|_| CoreError::invalid("catalog envelope is too large"))?
            || plan.review.prepared_at > now
        {
            pending.remove(&plan.review.action_id);
            return Err(CoreError::invalid("catalog import plan was changed"));
        }
        let state = self
            .storage()
            .catalog_state()
            .map_err(map_catalog_storage_error)?;
        if state.state_version != plan.review.expected_state_version {
            pending.remove(&plan.review.action_id);
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "catalog import plan is stale",
                true,
            ));
        }
        let candidate =
            build_catalog_import_candidate(self, envelope_json, now, plan.review.prepared_at)?;
        if !import_candidate_matches_review(&candidate, &plan.review) {
            pending.remove(&plan.review.action_id);
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "catalog import plan no longer matches the active catalog",
                true,
            ));
        }
        let result = commit_catalog_import_candidate(
            self,
            candidate,
            &plan.review.action_id,
            envelope_json,
            now,
        );
        pending.remove(&plan.review.action_id);
        drop(pending);
        result
    }

    /// Return the active catalog and monotonic signed-revision guard.
    pub fn provider_catalog_status(&self) -> CoreResult<ProviderCatalogStatus> {
        let state = self
            .storage()
            .catalog_state()
            .map_err(map_catalog_storage_error)?;
        let _accepted = load_verified_updates(self, &state)?;
        let active = load_active_catalog(self, &state)?;
        Ok(status_from_state(&state, &active))
    }

    /// Return the active provider/model view after evaluating freshness now.
    ///
    /// History APIs continue to expose the immutable snapshot accepted at the
    /// activation time. This operational view excludes expired signed layers
    /// and falls back to the bundled baseline without mutating history.
    pub fn active_provider_catalog_snapshot(&self) -> CoreResult<CatalogRevisionSnapshot> {
        Ok(self
            .operational_provider_catalog_projection_at(Utc::now())?
            .snapshot)
    }

    pub(crate) fn operational_provider_catalog_projection_at(
        &self,
        now: DateTime<Utc>,
    ) -> CoreResult<OperationalCatalogProjection> {
        operational_provider_catalog_projection_for_storage(self.storage(), now)
    }

    /// Return a bounded newest-first page of snapshot and activation history.
    pub fn provider_catalog_history(
        &self,
        limit: u32,
        before_revision: Option<u64>,
        before_state_version: Option<u64>,
    ) -> CoreResult<ProviderCatalogHistory> {
        if limit == 0 || limit > MAX_CATALOG_HISTORY_PAGE_SIZE {
            return Err(CoreError::invalid(
                "catalog history page size must be between 1 and 100",
            ));
        }
        let state = self
            .storage()
            .catalog_state()
            .map_err(map_catalog_storage_error)?;
        let _accepted = load_verified_updates(self, &state)?;
        let active = load_active_catalog(self, &state)?;
        if state.active.is_none() {
            let include_baseline = before_revision.is_none_or(|cursor| cursor > 1);
            return Ok(ProviderCatalogHistory {
                history_schema_version: PROVIDER_CATALOG_HISTORY_SCHEMA_VERSION,
                active_revision: 1,
                revisions: include_baseline
                    .then(|| revision_summary(&active.stored, 1))
                    .into_iter()
                    .collect(),
                activations: Vec::new(),
                next_before_revision: None,
                next_before_state_version: None,
            });
        }

        let fetch_limit = limit.saturating_add(1);
        let mut snapshots = self
            .storage()
            .catalog_snapshots(fetch_limit, before_revision)
            .map_err(map_catalog_storage_error)?;
        let more_snapshots = snapshots.len() > usize::try_from(limit).unwrap_or(usize::MAX);
        snapshots.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        let revisions = snapshots
            .iter()
            .map(|stored| {
                validate_stored_snapshot(stored)?;
                Ok(revision_summary(
                    stored,
                    state
                        .active
                        .as_ref()
                        .map_or(0, |pointer| pointer.local_revision),
                ))
            })
            .collect::<CoreResult<Vec<_>>>()?;
        let next_before_revision = more_snapshots
            .then(|| revisions.last().map(|revision| revision.revision))
            .flatten();

        let mut activations = self
            .storage()
            .catalog_activations(fetch_limit, before_state_version)
            .map_err(map_catalog_storage_error)?;
        let more_activations = activations.len() > usize::try_from(limit).unwrap_or(usize::MAX);
        activations.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        let activation_summaries = activations
            .iter()
            .map(activation_summary)
            .collect::<CoreResult<Vec<_>>>()?;
        let next_before_state_version = more_activations
            .then(|| {
                activation_summaries
                    .last()
                    .map(|activation| activation.state_version)
            })
            .flatten();

        Ok(ProviderCatalogHistory {
            history_schema_version: PROVIDER_CATALOG_HISTORY_SCHEMA_VERSION,
            active_revision: active.snapshot.revision,
            revisions,
            activations: activation_summaries,
            next_before_revision,
            next_before_state_version,
        })
    }

    /// Diff any two immutable local catalog revisions.
    pub fn diff_provider_catalog_revisions(
        &self,
        from_revision: u64,
        to_revision: u64,
    ) -> CoreResult<CatalogDiffDto> {
        let from = load_catalog_revision(self, from_revision)?;
        let to = load_catalog_revision(self, to_revision)?;
        from.snapshot
            .diff(&to.snapshot)
            .map_err(catalog_request_error)
    }

    /// Build a short-lived, state-bound rollback plan for native review.
    pub fn prepare_provider_catalog_rollback(
        &self,
        target_revision: u64,
    ) -> CoreResult<ProviderCatalogRollbackPlan> {
        let now = Utc::now();
        let state = self
            .storage()
            .catalog_state()
            .map_err(map_catalog_storage_error)?;
        let accepted = load_verified_updates(self, &state)?;
        let active = load_active_catalog(self, &state)?;
        let target = load_catalog_revision(self, target_revision)?;
        ensure_snapshot_operational(&target, &accepted, now)?;
        let history = rollback_history(&active.snapshot, &target.snapshot);
        let catalog_plan = history
            .prepare_rollback(target_revision, now)
            .map_err(catalog_request_error)?;
        let plan_sha256 = serialized_sha256(&catalog_plan)?;
        Ok(ProviderCatalogRollbackPlan {
            plan_schema_version: PROVIDER_CATALOG_ROLLBACK_PLAN_SCHEMA_VERSION,
            action_id: format!("catalog-rollback-{}", Uuid::new_v4()),
            expected_state_version: state.state_version,
            plan_sha256,
            catalog_plan,
        })
    }

    /// Activate one unmodified rollback plan after rechecking state and expiry.
    pub fn activate_provider_catalog_rollback(
        &self,
        plan: &ProviderCatalogRollbackPlan,
    ) -> CoreResult<ProviderCatalogRollbackResult> {
        if plan.plan_schema_version != PROVIDER_CATALOG_ROLLBACK_PLAN_SCHEMA_VERSION
            || plan.action_id.is_empty()
            || serialized_sha256(&plan.catalog_plan)? != plan.plan_sha256
        {
            return Err(CoreError::invalid("catalog rollback plan was changed"));
        }
        let now = Utc::now();
        let state = self
            .storage()
            .catalog_state()
            .map_err(map_catalog_storage_error)?;
        if state.state_version != plan.expected_state_version {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "catalog rollback plan is stale",
                true,
            ));
        }
        let accepted = load_verified_updates(self, &state)?;
        let active = load_active_catalog(self, &state)?;
        let target = load_catalog_revision(self, plan.catalog_plan.to_revision)?;
        ensure_snapshot_operational(&target, &accepted, now)?;
        let mut history = rollback_history(&active.snapshot, &target.snapshot);
        history
            .apply_rollback(&plan.catalog_plan, now)
            .map_err(catalog_request_error)?;
        let diff_json = serde_json::to_string(&plan.catalog_plan.diff)
            .map_err(|_| CoreError::internal("catalog diff could not be serialized"))?;
        let diff_sha256 = sha256_hex(diff_json.as_bytes());
        let audit_plan = CatalogRollbackAuditPlan {
            rollback_plan_version: CATALOG_ROLLBACK_AUDIT_PLAN_VERSION,
            from_revision: plan.catalog_plan.from_revision,
            to_revision: plan.catalog_plan.to_revision,
            expected_active_sha256: &plan.catalog_plan.expected_active_sha256,
            target_sha256: &plan.catalog_plan.target_sha256,
            created_at: plan.catalog_plan.created_at,
            expires_at: plan.catalog_plan.expires_at,
            diff_sha256: &diff_sha256,
        };
        let plan_json = serde_json::to_string(&audit_plan).map_err(|_| {
            CoreError::internal("catalog rollback audit plan could not be serialized")
        })?;
        let audit_plan_sha256 = sha256_hex(plan_json.as_bytes());
        let commit = CatalogRollbackCommit {
            expected: state.expectation(),
            action_id: &plan.action_id,
            target_local_revision: target.snapshot.revision,
            target_snapshot_sha256: &target.stored.snapshot_sha256,
            diff_json: &diff_json,
            rollback_plan_json: &plan_json,
            plan_sha256: &audit_plan_sha256,
            activated_at: now,
        };
        self.storage()
            .activate_catalog_rollback(&commit)
            .map_err(map_catalog_storage_error)?;
        let status = self.provider_catalog_status()?;
        Ok(ProviderCatalogRollbackResult {
            from_revision: active.snapshot.revision,
            activated_revision: target.snapshot.revision,
            status,
        })
    }
}

fn register_pending_catalog_import_plan(
    core: &Core,
    plan: &ProviderCatalogImportPlan,
    now: DateTime<Utc>,
) -> CoreResult<()> {
    let mut pending = core
        .pending_catalog_import_plans()
        .lock()
        .map_err(|_| CoreError::internal("catalog import review lock was poisoned"))?;
    pending.retain(|_, value| {
        value.expires_at > now && value.envelope_sha256 != plan.review.envelope_sha256
    });
    if pending.len() >= MAX_PENDING_CATALOG_IMPORT_PLANS {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "too many catalog import reviews are pending",
            true,
        ));
    }
    pending.insert(
        plan.review.action_id.clone(),
        PendingProviderCatalogImportPlan {
            plan_sha256: plan.plan_sha256.clone(),
            envelope_sha256: plan.review.envelope_sha256.clone(),
            expires_at: plan.review.expires_at,
        },
    );
    Ok(())
}

fn build_catalog_import_candidate(
    core: &Core,
    envelope_json: &[u8],
    validation_time: DateTime<Utc>,
    snapshot_captured_at: DateTime<Utc>,
) -> CoreResult<CatalogImportCandidate> {
    let state = core
        .storage()
        .catalog_state()
        .map_err(map_catalog_storage_error)?;
    let accepted = load_verified_updates(core, &state)?;
    let guard = provider_guard(&state.guard);
    let verified = verify_manual_catalog_import(envelope_json, &guard, validation_time)
        .map_err(catalog_request_error)?;
    let canonical_payload = verified
        .canonical_payload_json()
        .map_err(catalog_internal_error)?;
    let active = load_active_catalog(core, &state)?;
    let mut merge_updates =
        select_verified_updates(&accepted, &active.stored.signed_revision_chain)?;
    merge_updates.push(verified.clone());
    let merged = merge_with_bundled_catalog(
        &merge_updates,
        &[],
        validation_time,
        &CatalogFreshnessPolicy::default(),
    )
    .map_err(catalog_request_error)?;
    let next_local_revision = next_local_revision(core, &state)?;
    let snapshot =
        CatalogRevisionSnapshot::from_merged(next_local_revision, snapshot_captured_at, &merged)
            .map_err(catalog_request_error)?;
    ensure_catalog_template_versions_are_immutable(core, &snapshot)?;
    let snapshot_sha256 = snapshot.sha256().map_err(catalog_internal_error)?;
    let diff = active
        .snapshot
        .diff(&snapshot)
        .map_err(catalog_internal_error)?;
    let envelope: SignedCatalogEnvelope = serde_json::from_slice(envelope_json)
        .map_err(|_| CoreError::invalid("catalog envelope is malformed"))?;
    Ok(CatalogImportCandidate {
        state,
        active,
        verified,
        canonical_payload,
        envelope_version: envelope.envelope_version,
        envelope_sha256: sha256_hex(envelope_json),
        snapshot,
        snapshot_sha256,
        diff,
    })
}

fn import_candidate_matches_review(
    candidate: &CatalogImportCandidate,
    review: &ProviderCatalogImportReview,
) -> bool {
    review.plan_schema_version == PROVIDER_CATALOG_IMPORT_PLAN_SCHEMA_VERSION
        && candidate.state.state_version == review.expected_state_version
        && candidate.active.snapshot.revision == review.expected_active_revision
        && candidate.active.stored.snapshot_sha256 == review.expected_active_snapshot_sha256
        && candidate.state.guard.highest_accepted_revision
            == review.expected_highest_accepted_revision
        && candidate.envelope_sha256 == review.envelope_sha256
        && candidate.verified.signing_key_id() == review.signing_key_id
        && candidate.verified.payload_sha256() == review.payload_sha256
        && candidate.verified.payload().revision == review.signed_catalog_revision
        && candidate.snapshot.revision == review.candidate_revision
        && candidate.snapshot.captured_at == review.prepared_at
        && candidate.snapshot_sha256 == review.candidate_snapshot_sha256
        && review.expires_at
            == (review.prepared_at + Duration::minutes(CATALOG_IMPORT_PLAN_LIFETIME_MINUTES))
                .min(candidate.verified.payload().expires_at)
        && candidate.diff == review.diff
}

fn commit_catalog_import_candidate(
    core: &Core,
    candidate: CatalogImportCandidate,
    action_id: &str,
    envelope_json: &[u8],
    activated_at: DateTime<Utc>,
) -> CoreResult<ProviderCatalogImportResult> {
    let CatalogImportCandidate {
        state,
        active,
        verified,
        canonical_payload,
        envelope_version,
        envelope_sha256,
        snapshot,
        snapshot_sha256,
        diff,
    } = candidate;
    let diff_json = serde_json::to_string(&diff)
        .map_err(|_| CoreError::internal("catalog diff could not be serialized"))?;
    let (signing_key_id, payload_sha256, payload, next_guard) = verified.into_parts();
    let payload_json = String::from_utf8(canonical_payload)
        .map_err(|_| CoreError::internal("canonical catalog payload is not UTF-8"))?;
    let update_id = format!(
        "{}:{}:{}",
        payload.catalog_id,
        payload.revision,
        &payload_sha256[..16]
    );
    let mut signed_revision_chain = active.stored.signed_revision_chain.clone();
    signed_revision_chain.push(payload.revision);
    let baseline = if state.active.is_none() {
        Some(new_snapshot(&active.stored))
    } else {
        None
    };
    let snapshot_json = canonical_snapshot_json(&snapshot)?;
    let update = NewSignedCatalogUpdate {
        id: &update_id,
        catalog_id: &payload.catalog_id,
        catalog_schema_version: payload.schema_version,
        catalog_revision: payload.revision,
        envelope_version,
        signing_key_id: &signing_key_id,
        envelope: envelope_json,
        envelope_sha256: &envelope_sha256,
        payload_json: &payload_json,
        payload_sha256: &payload_sha256,
        issued_at: payload.issued_at,
        effective_at: payload.effective_at,
        expires_at: payload.expires_at,
        accepted_at: activated_at,
    };
    let new_snapshot = NewCatalogSnapshot {
        local_revision: snapshot.revision,
        snapshot_schema_version: snapshot.snapshot_schema_version,
        snapshot_json: &snapshot_json,
        snapshot_sha256: &snapshot_sha256,
        bundled_revision: active.stored.bundled_revision,
        bundled_sha256: &active.stored.bundled_sha256,
        signed_revision_chain: &signed_revision_chain,
        captured_at: snapshot.captured_at,
    };
    let next_guard = stored_guard(&next_guard);
    let commit = CatalogImportCommit {
        expected: state.expectation(),
        action_id,
        update,
        snapshot: new_snapshot,
        initial_baseline: baseline,
        next_guard,
        diff_json: &diff_json,
        activated_at,
    };
    core.storage()
        .commit_catalog_import(&commit)
        .map_err(map_catalog_storage_error)?;
    let status = core.provider_catalog_status()?;
    Ok(ProviderCatalogImportResult {
        signed_catalog_revision: payload.revision,
        activated_revision: snapshot.revision,
        diff,
        status,
    })
}

impl OperationalCatalogProjection {
    pub(crate) fn provider_templates(&self) -> Vec<ProviderTemplate> {
        self.merged
            .manifests
            .iter()
            .map(|manifest| manifest.entry.template.clone())
            .collect()
    }

    pub(crate) fn provider_template(
        &self,
        id: &ProviderTemplateId,
        version: u32,
    ) -> Option<ProviderTemplate> {
        self.merged
            .manifests
            .iter()
            .find(|manifest| {
                manifest.entry.template.id == *id
                    && manifest.entry.template.manifest_version == version
            })
            .map(|manifest| manifest.entry.template.clone())
    }

    pub(crate) fn discovery_authority_binding(
        &self,
        template: &ProviderTemplate,
        now: DateTime<Utc>,
    ) -> CoreResult<Option<DiscoveryCatalogAuthorityBinding>> {
        if template.source != lorepia_domain::TemplateSource::SignedCatalog {
            return Ok(None);
        }
        let manifest = self
            .merged
            .manifests
            .iter()
            .find(|manifest| {
                manifest.entry.template.id == template.id
                    && manifest.entry.template.manifest_version == template.manifest_version
            })
            .ok_or_else(stale_discovery_catalog_authority)?;
        if manifest.entry.template != *template
            || manifest.provenance.authority != CatalogAuthority::SignedCatalog
        {
            return Err(stale_discovery_catalog_authority());
        }
        let payload_expires_at = self
            .signed_layer_expirations
            .get(&manifest.provenance.layer_id)
            .copied()
            .ok_or_else(stale_discovery_catalog_authority)?;
        let expires_at = manifest
            .entry
            .expires_at
            .map_or(payload_expires_at, |entry_expiry| {
                entry_expiry.min(payload_expires_at)
            });
        if expires_at <= now {
            return Err(stale_discovery_catalog_authority());
        }
        DiscoveryCatalogAuthorityBinding::new(self.state_version, template, expires_at)
            .map(Some)
            .map_err(|_| stale_discovery_catalog_authority())
    }

    pub(crate) fn route_projection(
        &self,
        route: &ModelRoute,
        provider_template_id: &ProviderTemplateId,
    ) -> CatalogRouteProjection {
        project_catalog_route(
            self.snapshot.revision,
            &self.merged.models,
            &self.signed_layer_expirations,
            route,
            provider_template_id,
        )
    }
}

fn stale_discovery_catalog_authority() -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        "signed catalog authority changed or expired; restart provider discovery",
        true,
    )
}

pub(crate) fn operational_provider_catalog_projection_for_storage(
    storage: &Storage,
    now: DateTime<Utc>,
) -> CoreResult<OperationalCatalogProjection> {
    let state = storage.catalog_state().map_err(map_catalog_storage_error)?;
    let accepted = load_verified_updates_from_storage(storage, &state)?;
    let active = load_active_catalog_from_storage(storage, &state)?;
    let updates = select_verified_updates(&accepted, &active.stored.signed_revision_chain)?;
    let signed_layer_expirations = updates
        .iter()
        .map(|update| {
            (
                format!(
                    "signed:{}:{}",
                    update.signing_key_id(),
                    update.payload().revision
                ),
                update.payload().expires_at,
            )
        })
        .collect();
    let merged = merge_with_bundled_catalog(&updates, &[], now, &CatalogFreshnessPolicy::default())
        .map_err(catalog_request_error)?;
    let snapshot = CatalogRevisionSnapshot::from_merged(
        active.snapshot.revision,
        active.snapshot.captured_at,
        &merged,
    )
    .map_err(catalog_internal_error)?;
    Ok(OperationalCatalogProjection {
        state_version: state.state_version,
        snapshot,
        merged,
        signed_layer_expirations,
    })
}

#[allow(clippy::too_many_lines)]
fn project_catalog_route(
    catalog_revision: u64,
    catalog_models: &[MergedCatalogModel],
    signed_layer_expirations: &BTreeMap<String, DateTime<Utc>>,
    route: &ModelRoute,
    provider_template_id: &ProviderTemplateId,
) -> CatalogRouteProjection {
    let mut matching = catalog_models
        .iter()
        .filter(|model| {
            model.entry.provider_template_id == *provider_template_id
                && model.entry.api_family == route.api_family
                && model.entry.model_match.matches(&route.model_id)
        })
        .collect::<Vec<_>>();
    matching.sort_by(|left, right| {
        model_match_priority(&left.entry.model_match)
            .cmp(&model_match_priority(&right.entry.model_match))
            .then_with(|| {
                left.entry
                    .metadata_version
                    .cmp(&right.entry.metadata_version)
            })
            .then_with(|| left.entry.verified_at.cmp(&right.entry.verified_at))
            .then_with(|| left.entry.id.cmp(&right.entry.id))
    });
    if matching.is_empty() {
        return CatalogRouteProjection::default();
    }

    let mut parameters = BTreeMap::<String, ParameterSpec>::new();
    let mut signed_parameters = BTreeMap::<String, ParameterSpec>::new();
    let mut capabilities = BTreeMap::<CatalogCapabilityKey, CapabilityObservation>::new();
    for model in matching {
        for parameter in &model.entry.parameters {
            let Some(provenance) = model.parameter_provenance.get(parameter.id.as_str()) else {
                continue;
            };
            // Compiled mappings remain valid with the binary. A stale signed
            // mapping remains visible in history but cannot change a request.
            if provenance.authority == CatalogAuthority::Bundled
                || provenance.freshness == CatalogFreshness::Current
            {
                let id = parameter.id.as_str().to_owned();
                parameters.insert(id.clone(), parameter.clone());
                if provenance.authority == CatalogAuthority::SignedCatalog
                    && provenance.freshness == CatalogFreshness::Current
                    && !matches!(&model.entry.model_match, ModelMatch::AnyModel)
                {
                    signed_parameters.insert(id, parameter.clone());
                } else {
                    signed_parameters.remove(&id);
                }
            }
        }
        for capability in &model.entry.capabilities {
            let Some(provenance) = model.capability_provenance.get(&capability.key) else {
                continue;
            };
            if provenance.authority != CatalogAuthority::SignedCatalog {
                continue;
            }
            let Some((key, value, status)) =
                catalog_capability_value(capability.key, &capability.value)
            else {
                continue;
            };
            let observation_identity = format!(
                "lorepia:signed-catalog-capability:v1\u{0}{catalog_revision}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
                route.id.as_str(),
                model.entry.id,
                catalog_capability_key_identity(capability.key),
                provenance.layer_id,
                provenance.revision,
            );
            let evidence_identity = format!(
                "lorepia:signed-catalog-evidence:v1\u{0}{catalog_revision}\u{0}{}\u{0}{}\u{0}{}",
                model.entry.id,
                provenance.layer_id,
                catalog_capability_key_identity(capability.key),
            );
            let freshness_policy = CatalogFreshnessPolicy::default();
            let stale_at = provenance
                .verified_at
                .checked_add_signed(chrono::Duration::seconds(
                    i64::try_from(freshness_policy.signed_max_age_seconds).unwrap_or(i64::MAX),
                ));
            let expires_at = minimum_time(
                minimum_time(model.entry.expires_at, stale_at),
                signed_layer_expirations.get(&provenance.layer_id).copied(),
            );
            capabilities.insert(
                capability.key,
                CapabilityObservation {
                    id: ObservationId::from(
                        Uuid::new_v5(&Uuid::NAMESPACE_URL, observation_identity.as_bytes())
                            .to_string(),
                    ),
                    model_route_id: route.id.clone(),
                    key,
                    value,
                    status,
                    source: ObservationSource::SignedLorepiaCatalog,
                    confidence: Confidence::High,
                    observed_at: provenance.verified_at,
                    expires_at,
                    evidence_ref: Some(EvidenceId::from(
                        Uuid::new_v5(&Uuid::NAMESPACE_URL, evidence_identity.as_bytes())
                            .to_string(),
                    )),
                },
            );
        }
    }

    CatalogRouteProjection {
        matched: true,
        parameters: parameters.into_values().collect(),
        signed_parameters: signed_parameters.into_values().collect(),
        capability_observations: capabilities.into_values().collect(),
    }
}

fn ensure_catalog_template_versions_are_immutable(
    core: &Core,
    snapshot: &CatalogRevisionSnapshot,
) -> CoreResult<()> {
    let mut existing = core
        .storage()
        .list_provider_templates()?
        .into_iter()
        .map(|template| ((template.id.clone(), template.manifest_version), template))
        .collect::<BTreeMap<_, _>>();
    let mut before_revision = None;
    loop {
        let page = core
            .storage()
            .catalog_snapshots(CATALOG_STORAGE_PAGE_SIZE, before_revision)
            .map_err(map_catalog_storage_error)?;
        if page.is_empty() {
            break;
        }
        before_revision = page.last().map(|stored| stored.local_revision);
        let page_full =
            page.len() == usize::try_from(CATALOG_STORAGE_PAGE_SIZE).unwrap_or(usize::MAX);
        for stored in page {
            let historical = validate_stored_snapshot(&stored)?;
            for manifest in historical.manifests {
                let template = manifest.template;
                let key = (template.id.clone(), template.manifest_version);
                if existing
                    .get(&key)
                    .is_some_and(|previous| previous != &template)
                {
                    return Err(CoreError::invalid(
                        "provider catalog history reuses an immutable template version with different content",
                    ));
                }
                existing.insert(key, template);
            }
        }
        if !page_full {
            break;
        }
    }
    for manifest in &snapshot.manifests {
        let template = &manifest.template;
        if existing
            .get(&(template.id.clone(), template.manifest_version))
            .is_some_and(|stored| stored != template)
        {
            return Err(CoreError::invalid(
                "signed catalog reuses an immutable provider template version with different content",
            ));
        }
    }
    Ok(())
}

fn model_match_priority(model_match: &ModelMatch) -> (u8, usize) {
    match model_match {
        ModelMatch::AnyModel => (0, 0),
        ModelMatch::Glob { pattern } => (1, pattern.len().saturating_sub(1)),
        ModelMatch::Exact { model_id } => (2, model_id.len()),
    }
}

fn catalog_capability_value(
    key: CatalogCapabilityKey,
    value: &CatalogCapabilityValue,
) -> Option<(CapabilityKey, CapabilityValue, SupportStatus)> {
    let key = match key {
        CatalogCapabilityKey::Streaming => CapabilityKey::Streaming,
        CatalogCapabilityKey::Reasoning => CapabilityKey::Reasoning,
        CatalogCapabilityKey::StructuredOutput => CapabilityKey::StructuredOutput,
        CatalogCapabilityKey::ToolCalling => CapabilityKey::ToolCalling,
        CatalogCapabilityKey::PromptCaching => CapabilityKey::PromptCaching,
        CatalogCapabilityKey::ContextTokens => CapabilityKey::ContextWindow,
        CatalogCapabilityKey::MaxOutputTokens => CapabilityKey::MaxOutputTokens,
    };
    let (value, status) = match value {
        CatalogCapabilityValue::Unknown => return None,
        CatalogCapabilityValue::Boolean(value) => (
            CapabilityValue::Boolean(*value),
            if *value {
                SupportStatus::Documented
            } else {
                SupportStatus::Unsupported
            },
        ),
        CatalogCapabilityValue::Integer(value) => {
            (CapabilityValue::Integer(*value), SupportStatus::Documented)
        }
        CatalogCapabilityValue::EnumValues(values) => (
            CapabilityValue::EnumValues(values.clone()),
            SupportStatus::Documented,
        ),
    };
    Some((key, value, status))
}

const fn catalog_capability_key_identity(key: CatalogCapabilityKey) -> &'static str {
    match key {
        CatalogCapabilityKey::Streaming => "streaming",
        CatalogCapabilityKey::Reasoning => "reasoning",
        CatalogCapabilityKey::StructuredOutput => "structured_output",
        CatalogCapabilityKey::ToolCalling => "tool_calling",
        CatalogCapabilityKey::PromptCaching => "prompt_caching",
        CatalogCapabilityKey::ContextTokens => "context_tokens",
        CatalogCapabilityKey::MaxOutputTokens => "max_output_tokens",
    }
}

fn minimum_time(
    left: Option<DateTime<Utc>>,
    right: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn load_verified_updates(
    core: &Core,
    state: &StoredCatalogState,
) -> CoreResult<BTreeMap<u64, VerifiedCatalogUpdate>> {
    load_verified_updates_from_storage(core.storage(), state)
}

fn load_verified_updates_from_storage(
    storage: &Storage,
    state: &StoredCatalogState,
) -> CoreResult<BTreeMap<u64, VerifiedCatalogUpdate>> {
    let mut stored = Vec::new();
    let mut before = None;
    loop {
        let page = storage
            .catalog_updates(CATALOG_STORAGE_PAGE_SIZE, before)
            .map_err(map_catalog_storage_error)?;
        if page.is_empty() {
            break;
        }
        before = page.last().map(|update| update.catalog_revision);
        let page_full =
            page.len() == usize::try_from(CATALOG_STORAGE_PAGE_SIZE).unwrap_or(usize::MAX);
        stored.extend(page);
        if !page_full {
            break;
        }
    }
    if u64::try_from(stored.len()).ok() != Some(state.update_count) {
        return Err(catalog_storage_error(
            "catalog update count changed during verification",
        ));
    }
    stored.sort_by_key(|update| update.catalog_revision);
    let mut guard = CatalogRevisionGuard::default();
    let mut verified_by_revision = BTreeMap::new();
    for update in stored {
        verify_stored_hashes(&update)?;
        let verified = verify_stored_catalog_update(&update.envelope, &guard, update.accepted_at)
            .map_err(|_| catalog_storage_error("stored catalog signature is invalid"))?;
        verify_stored_update_metadata(&update, &verified)?;
        guard = verified.next_revision_guard().clone();
        if verified_by_revision
            .insert(update.catalog_revision, verified)
            .is_some()
        {
            return Err(catalog_storage_error(
                "stored catalog revision is duplicated",
            ));
        }
    }
    if guard != provider_guard(&state.guard) {
        return Err(catalog_storage_error(
            "stored catalog revision guard does not match accepted updates",
        ));
    }
    Ok(verified_by_revision)
}

fn verify_stored_hashes(update: &StoredSignedCatalogUpdate) -> CoreResult<()> {
    if sha256_hex(&update.envelope) != update.envelope_sha256
        || sha256_hex(update.payload_json.as_bytes()) != update.payload_sha256
    {
        return Err(catalog_storage_error(
            "stored catalog bytes do not match their hashes",
        ));
    }
    Ok(())
}

fn verify_stored_update_metadata(
    stored: &StoredSignedCatalogUpdate,
    verified: &VerifiedCatalogUpdate,
) -> CoreResult<()> {
    let payload = verified.payload();
    let canonical = verified
        .canonical_payload_json()
        .map_err(catalog_internal_error)?;
    if verified.signing_key_id() != stored.signing_key_id
        || verified.payload_sha256() != stored.payload_sha256
        || canonical != stored.payload_json.as_bytes()
        || payload.catalog_id != stored.catalog_id
        || payload.schema_version != stored.catalog_schema_version
        || payload.revision != stored.catalog_revision
        || payload.issued_at != stored.issued_at
        || payload.effective_at != stored.effective_at
        || payload.expires_at != stored.expires_at
        || stored.accepted_guard != stored_guard(verified.next_revision_guard())
    {
        return Err(catalog_storage_error(
            "stored catalog metadata does not match its signed payload",
        ));
    }
    Ok(())
}

fn select_verified_updates(
    accepted: &BTreeMap<u64, VerifiedCatalogUpdate>,
    revisions: &[u64],
) -> CoreResult<Vec<VerifiedCatalogUpdate>> {
    revisions
        .iter()
        .map(|revision| {
            accepted.get(revision).cloned().ok_or_else(|| {
                catalog_storage_error("catalog snapshot references an unknown signed update")
            })
        })
        .collect()
}

fn load_active_catalog(core: &Core, state: &StoredCatalogState) -> CoreResult<ActiveCatalog> {
    load_active_catalog_from_storage(core.storage(), state)
}

fn load_active_catalog_from_storage(
    storage: &Storage,
    state: &StoredCatalogState,
) -> CoreResult<ActiveCatalog> {
    if let Some(pointer) = &state.active {
        let stored = storage
            .catalog_snapshot(pointer.local_revision)
            .map_err(map_catalog_storage_error)?
            .ok_or_else(|| catalog_storage_error("active catalog snapshot is missing"))?;
        if stored.snapshot_sha256 != pointer.snapshot_sha256 {
            return Err(catalog_storage_error(
                "active catalog pointer hash does not match",
            ));
        }
        let snapshot = validate_stored_snapshot(&stored)?;
        ensure_current_baseline(&stored)?;
        Ok(ActiveCatalog { stored, snapshot })
    } else {
        if state.state_version != 0
            || state.snapshot_count != 0
            || state.update_count != 0
            || state.activation_count != 0
            || state.guard.highest_accepted_revision != 0
            || state.guard.latest_issued_at.is_some()
        {
            return Err(catalog_storage_error(
                "uninitialized catalog state contains durable history",
            ));
        }
        let snapshot = bundled_baseline_snapshot()?;
        let stored = synthetic_baseline(&snapshot)?;
        Ok(ActiveCatalog { stored, snapshot })
    }
}

fn load_catalog_revision(core: &Core, revision: u64) -> CoreResult<ActiveCatalog> {
    let state = core
        .storage()
        .catalog_state()
        .map_err(map_catalog_storage_error)?;
    if state.active.is_none() && revision == 1 {
        return load_active_catalog(core, &state);
    }
    let stored = core
        .storage()
        .catalog_snapshot(revision)
        .map_err(map_catalog_storage_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "catalog revision was not found",
                false,
            )
        })?;
    let snapshot = validate_stored_snapshot(&stored)?;
    ensure_current_baseline(&stored)?;
    Ok(ActiveCatalog { stored, snapshot })
}

fn validate_stored_snapshot(stored: &StoredCatalogSnapshot) -> CoreResult<CatalogRevisionSnapshot> {
    let snapshot: CatalogRevisionSnapshot = serde_json::from_str(&stored.snapshot_json)
        .map_err(|_| catalog_storage_error("stored catalog snapshot is malformed"))?;
    let canonical = snapshot.canonical_json().map_err(|_| {
        catalog_storage_error("stored catalog snapshot violates catalog invariants")
    })?;
    let sha256 = snapshot
        .sha256()
        .map_err(|_| catalog_storage_error("stored catalog snapshot cannot be hashed"))?;
    CatalogHistory::new(snapshot.clone())
        .map_err(|_| catalog_storage_error("stored catalog snapshot is invalid"))?;
    if canonical != stored.snapshot_json.as_bytes()
        || sha256 != stored.snapshot_sha256
        || snapshot.revision != stored.local_revision
        || snapshot.snapshot_schema_version != stored.snapshot_schema_version
        || snapshot.captured_at != stored.captured_at
    {
        return Err(catalog_storage_error(
            "stored catalog snapshot metadata does not match",
        ));
    }
    Ok(snapshot)
}

fn ensure_current_baseline(stored: &StoredCatalogSnapshot) -> CoreResult<()> {
    let baseline = bundled_baseline_snapshot()?;
    let baseline_hash = baseline.sha256().map_err(catalog_internal_error)?;
    if stored.bundled_revision != BUNDLED_CATALOG_REVISION || stored.bundled_sha256 != baseline_hash
    {
        return Err(catalog_storage_error(
            "stored catalog was built with a different bundled baseline",
        ));
    }
    Ok(())
}

fn synthetic_baseline(snapshot: &CatalogRevisionSnapshot) -> CoreResult<StoredCatalogSnapshot> {
    let snapshot_json = canonical_snapshot_json(snapshot)?;
    let snapshot_sha256 = snapshot.sha256().map_err(catalog_internal_error)?;
    Ok(StoredCatalogSnapshot {
        local_revision: snapshot.revision,
        snapshot_schema_version: snapshot.snapshot_schema_version,
        snapshot_json,
        snapshot_sha256: snapshot_sha256.clone(),
        bundled_revision: BUNDLED_CATALOG_REVISION,
        bundled_sha256: snapshot_sha256,
        signed_revision_chain: Vec::new(),
        source: CatalogSnapshotSource::BundledBaseline,
        captured_at: snapshot.captured_at,
    })
}

fn next_local_revision(core: &Core, state: &StoredCatalogState) -> CoreResult<u64> {
    let maximum = if state.snapshot_count == 0 {
        1
    } else {
        core.storage()
            .catalog_snapshots(1, None)
            .map_err(map_catalog_storage_error)?
            .first()
            .map_or(0, |snapshot| snapshot.local_revision)
    };
    maximum
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("catalog revision history is full"))
}

fn new_snapshot(stored: &StoredCatalogSnapshot) -> NewCatalogSnapshot<'_> {
    NewCatalogSnapshot {
        local_revision: stored.local_revision,
        snapshot_schema_version: stored.snapshot_schema_version,
        snapshot_json: &stored.snapshot_json,
        snapshot_sha256: &stored.snapshot_sha256,
        bundled_revision: stored.bundled_revision,
        bundled_sha256: &stored.bundled_sha256,
        signed_revision_chain: &stored.signed_revision_chain,
        captured_at: stored.captured_at,
    }
}

fn canonical_snapshot_json(snapshot: &CatalogRevisionSnapshot) -> CoreResult<String> {
    String::from_utf8(snapshot.canonical_json().map_err(catalog_internal_error)?)
        .map_err(|_| CoreError::internal("canonical catalog snapshot is not UTF-8"))
}

fn provider_guard(guard: &StoredCatalogRevisionGuard) -> CatalogRevisionGuard {
    CatalogRevisionGuard {
        highest_accepted_revision: guard.highest_accepted_revision,
        latest_issued_at: guard.latest_issued_at,
    }
}

fn stored_guard(guard: &CatalogRevisionGuard) -> StoredCatalogRevisionGuard {
    StoredCatalogRevisionGuard {
        highest_accepted_revision: guard.highest_accepted_revision,
        latest_issued_at: guard.latest_issued_at,
    }
}

fn status_from_state(state: &StoredCatalogState, active: &ActiveCatalog) -> ProviderCatalogStatus {
    ProviderCatalogStatus {
        status_schema_version: PROVIDER_CATALOG_STATUS_SCHEMA_VERSION,
        state_version: state.state_version,
        active_revision: active.snapshot.revision,
        active_snapshot_sha256: active.stored.snapshot_sha256.clone(),
        bundled_baseline_sha256: active.stored.bundled_sha256.clone(),
        snapshot_count: u32::try_from(state.snapshot_count.max(1)).unwrap_or(u32::MAX),
        signed_update_count: u32::try_from(state.update_count).unwrap_or(u32::MAX),
        highest_accepted_revision: state.guard.highest_accepted_revision,
        latest_issued_at: state.guard.latest_issued_at,
        active_signed_revisions: active.stored.signed_revision_chain.clone(),
    }
}

fn revision_summary(
    stored: &StoredCatalogSnapshot,
    active_revision: u64,
) -> ProviderCatalogRevisionSummary {
    ProviderCatalogRevisionSummary {
        revision: stored.local_revision,
        captured_at: stored.captured_at,
        snapshot_sha256: stored.snapshot_sha256.clone(),
        signed_revisions: stored.signed_revision_chain.clone(),
        active: stored.local_revision == active_revision,
    }
}

fn activation_summary(
    record: &CatalogActivationRecord,
) -> CoreResult<ProviderCatalogActivationSummary> {
    let diff: CatalogDiffDto = serde_json::from_str(&record.diff_json)
        .map_err(|_| catalog_storage_error("stored catalog activation diff is malformed"))?;
    if diff.from_revision != record.from.as_ref().map_or(0, |from| from.local_revision)
        || diff.to_revision != record.to.local_revision
    {
        return Err(catalog_storage_error(
            "stored catalog activation diff does not match its audit row",
        ));
    }
    Ok(ProviderCatalogActivationSummary {
        action_id: record.action_id.clone(),
        state_version: record.state_version,
        kind: match record.kind {
            CatalogActivationKind::Import => ProviderCatalogActivationKind::Import,
            CatalogActivationKind::Rollback => ProviderCatalogActivationKind::Rollback,
        },
        from_revision: record.from.as_ref().map(|from| from.local_revision),
        to_revision: record.to.local_revision,
        activated_at: record.activated_at,
        diff,
    })
}

fn ensure_snapshot_operational(
    target: &ActiveCatalog,
    accepted: &BTreeMap<u64, VerifiedCatalogUpdate>,
    now: DateTime<Utc>,
) -> CoreResult<()> {
    let updates = select_verified_updates(accepted, &target.stored.signed_revision_chain)?;
    let merged = merge_with_bundled_catalog(&updates, &[], now, &CatalogFreshnessPolicy::default())
        .map_err(catalog_request_error)?;
    let current_view = CatalogRevisionSnapshot::from_merged(
        target.snapshot.revision,
        target.snapshot.captured_at,
        &merged,
    )
    .map_err(catalog_request_error)?;
    if current_view.sha256().map_err(catalog_internal_error)? != target.stored.snapshot_sha256 {
        return Err(CoreError::invalid(
            "catalog rollback target contains metadata that is now expired",
        ));
    }
    Ok(())
}

fn rollback_history(
    active: &CatalogRevisionSnapshot,
    target: &CatalogRevisionSnapshot,
) -> CatalogHistory {
    CatalogHistory {
        history_schema_version: CATALOG_HISTORY_SCHEMA_VERSION,
        active_revision: active.revision,
        snapshots: vec![active.clone(), target.clone()],
    }
}

fn map_catalog_storage_error(error: CatalogStorageError) -> CoreError {
    match error {
        CatalogStorageError::InvalidInput(message) => CoreError::invalid(message),
        CatalogStorageError::Corrupted(message) => catalog_storage_error(message),
        CatalogStorageError::StateConflict => CoreError::new(
            CoreErrorCode::InvalidInput,
            "catalog state changed; review the operation again",
            true,
        ),
        CatalogStorageError::NotFound(item) => CoreError::new(
            CoreErrorCode::NotFound,
            format!("catalog {item} was not found"),
            false,
        ),
        CatalogStorageError::StorageUnavailable => CoreError::new(
            CoreErrorCode::StorageUnavailable,
            "catalog storage is unavailable",
            true,
        ),
        CatalogStorageError::Database(_) => CoreError::new(
            CoreErrorCode::StorageUnavailable,
            "catalog database operation failed",
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CoreConfig;
    use lorepia_domain::{
        ApiFamily, ModelAvailability, ModelMetadataSource, ModelRouteConfig, ProviderConnectionId,
    };
    use lorepia_providers::{AdapterRegistry, BuiltInTemplateId};

    fn invalid_envelope(signing_key_id: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "envelope_version": 1,
            "signing_key_id": signing_key_id,
            "payload_base64": "e30=",
            "signature_base64": format!("{}==", "A".repeat(86)),
        }))
        .expect("serialize invalid envelope")
    }

    fn register_fake_import_plan(
        core: &Core,
        envelope: &[u8],
        prepared_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> ProviderCatalogImportPlan {
        let status = core.provider_catalog_status().expect("catalog status");
        let action_id = format!("catalog-import-{}", Uuid::new_v4());
        let review = ProviderCatalogImportReview {
            plan_schema_version: PROVIDER_CATALOG_IMPORT_PLAN_SCHEMA_VERSION,
            action_id: action_id.clone(),
            expected_state_version: status.state_version,
            expected_active_revision: status.active_revision,
            expected_active_snapshot_sha256: status.active_snapshot_sha256,
            expected_highest_accepted_revision: status.highest_accepted_revision,
            envelope_byte_count: u64::try_from(envelope.len()).expect("envelope length"),
            envelope_sha256: sha256_hex(envelope),
            signing_key_id: "lorepia-catalog-2026-01".to_owned(),
            payload_sha256: "a".repeat(64),
            signed_catalog_revision: 2,
            candidate_revision: 2,
            candidate_snapshot_sha256: "b".repeat(64),
            prepared_at,
            expires_at,
            diff: core
                .diff_provider_catalog_revisions(1, 1)
                .expect("baseline diff"),
        };
        let plan_sha256 = serialized_sha256(&review).expect("plan hash");
        let plan = ProviderCatalogImportPlan {
            review,
            plan_sha256,
        };
        register_pending_catalog_import_plan(core, &plan, prepared_at)
            .expect("register pending catalog import plan");
        plan
    }

    #[test]
    fn fresh_catalog_status_history_and_diff_are_deterministic_after_reopen() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config = CoreConfig::new(directory.path());
        let core = Core::open(config.clone()).expect("open core");
        let status = core.provider_catalog_status().expect("catalog status");
        assert_eq!(status.state_version, 0);
        assert_eq!(status.active_revision, 1);
        assert_eq!(status.snapshot_count, 1);
        assert_eq!(status.signed_update_count, 0);
        assert_eq!(status.highest_accepted_revision, 0);
        assert!(status.active_signed_revisions.is_empty());

        let history = core
            .provider_catalog_history(20, None, None)
            .expect("catalog history");
        assert_eq!(history.active_revision, 1);
        assert_eq!(history.revisions.len(), 1);
        assert!(history.revisions[0].active);
        assert!(history.activations.is_empty());
        let diff = core
            .diff_provider_catalog_revisions(1, 1)
            .expect("baseline self diff");
        assert!(diff.manifest_changes.is_empty());
        assert!(diff.model_changes.is_empty());
        drop(core);

        let reopened = Core::open(config).expect("reopen core");
        assert_eq!(
            reopened
                .provider_catalog_status()
                .expect("reopened catalog status"),
            status
        );
    }

    #[test]
    fn discovery_authority_binds_state_expiry_and_exact_signed_template() {
        let now = DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let expires_at = now + Duration::minutes(10);
        let mut template =
            AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter).unwrap();
        template.source = lorepia_domain::TemplateSource::SignedCatalog;
        template.manifest_version += 1;
        let entry = lorepia_providers::catalog::CatalogManifestEntry {
            template: template.clone(),
            verified_at: now - Duration::minutes(1),
            expires_at: None,
            sources: Vec::new(),
        };
        let layer_id = "signed:test-key:7".to_owned();
        let projection = OperationalCatalogProjection {
            state_version: 7,
            snapshot: CatalogRevisionSnapshot {
                snapshot_schema_version: CATALOG_HISTORY_SCHEMA_VERSION,
                revision: 2,
                captured_at: now,
                manifests: vec![entry.clone()],
                models: Vec::new(),
            },
            merged: MergedCatalog {
                manifests: vec![lorepia_providers::catalog::MergedCatalogManifest {
                    entry,
                    provenance: lorepia_providers::catalog::CatalogFieldProvenance {
                        authority: CatalogAuthority::SignedCatalog,
                        layer_id: layer_id.clone(),
                        revision: 7,
                        verified_at: now - Duration::minutes(1),
                        freshness: CatalogFreshness::Current,
                    },
                }],
                models: Vec::new(),
                report: lorepia_providers::catalog::CatalogMergeReport::default(),
            },
            signed_layer_expirations: BTreeMap::from([(layer_id, expires_at)]),
        };

        let binding = projection
            .discovery_authority_binding(&template, now)
            .unwrap()
            .unwrap();
        assert_eq!(binding.catalog_state_version, 7);
        assert_eq!(binding.expires_at, expires_at);

        let mut changed_state = projection.clone();
        changed_state.state_version += 1;
        assert_ne!(
            changed_state
                .discovery_authority_binding(&template, now)
                .unwrap()
                .unwrap(),
            binding
        );

        let mut replaced = template.clone();
        replaced.default_manifest.endpoints.generate.path =
            lorepia_domain::EndpointPath::parse("/replacement").unwrap();
        assert!(
            projection
                .discovery_authority_binding(&replaced, now)
                .is_err()
        );
        assert!(
            projection
                .discovery_authority_binding(&template, expires_at)
                .is_err()
        );
    }

    #[test]
    fn unknown_key_and_invalid_signature_leave_catalog_state_unchanged() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let core = Core::open(CoreConfig::new(directory.path())).expect("open core");
        let before = core.provider_catalog_status().expect("initial status");

        let unknown = core
            .prepare_signed_provider_catalog_import(&invalid_envelope("unknown-catalog-key"))
            .expect_err("unknown key must fail");
        assert_eq!(unknown.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            core.provider_catalog_status()
                .expect("status after unknown key"),
            before
        );

        let invalid_signature = core
            .prepare_signed_provider_catalog_import(&invalid_envelope("lorepia-catalog-2026-01"))
            .expect_err("invalid signature must fail");
        assert_eq!(invalid_signature.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            core.provider_catalog_status()
                .expect("status after invalid signature"),
            before
        );
    }

    #[test]
    fn import_review_rejects_tamper_expiry_and_reuse_without_exposing_envelope() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let core = Core::open(CoreConfig::new(directory.path())).expect("open core");
        let now = Utc::now();
        let envelope = b"catalog-envelope-credential-canary";
        let status_before_prepare = core
            .provider_catalog_status()
            .expect("status before review");
        let plan = register_fake_import_plan(
            &core,
            envelope,
            now,
            now + Duration::minutes(CATALOG_IMPORT_PLAN_LIFETIME_MINUTES),
        );
        assert_eq!(
            core.provider_catalog_status()
                .expect("status after review registration"),
            status_before_prepare,
            "review registration must not activate or persist catalog content"
        );
        assert!(!format!("{plan:?}").contains("credential-canary"));

        let mut tampered = plan.clone();
        tampered.review.candidate_revision += 1;
        let changed = core
            .activate_signed_provider_catalog_import(&tampered, envelope)
            .expect_err("tampered plan must fail before envelope parsing");
        assert_eq!(changed.code, CoreErrorCode::InvalidInput);
        assert_eq!(changed.message, "catalog import plan was changed");

        let reused = core
            .activate_signed_provider_catalog_import(&plan, envelope)
            .expect_err("removed plan cannot be reused");
        assert_eq!(reused.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            reused.message,
            "catalog import plan is unknown or was already used"
        );

        let expired = register_fake_import_plan(
            &core,
            envelope,
            now - Duration::minutes(CATALOG_IMPORT_PLAN_LIFETIME_MINUTES + 1),
            now - Duration::minutes(1),
        );
        let expired_error = core
            .activate_signed_provider_catalog_import(&expired, envelope)
            .expect_err("expired plan must fail before envelope parsing");
        assert_eq!(expired_error.code, CoreErrorCode::InvalidInput);
        assert_eq!(expired_error.message, "catalog import plan has expired");
    }

    #[test]
    fn catalog_history_rejects_unbounded_page_requests() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let core = Core::open(CoreConfig::new(directory.path())).expect("open core");
        let error = core
            .provider_catalog_history(MAX_CATALOG_HISTORY_PAGE_SIZE + 1, None, None)
            .expect_err("oversized page must fail");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
    }

    fn catalog_projection_route(now: DateTime<Utc>) -> ModelRoute {
        ModelRoute {
            id: lorepia_domain::ModelRouteId::from("catalog-projection-route"),
            connection_id: ProviderConnectionId::from("catalog-projection-connection"),
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: "example-reasoning-model".to_owned(),
            display_name: None,
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::SignedCatalog,
            metadata_observed_at: Some(now),
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: now,
            last_seen_at: Some(now),
        }
    }

    fn catalog_projection_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the test keeps current and stale field provenance in one ordered route projection"
    )]
    fn route_projection_applies_specific_fresh_parameters_and_marks_stale_capabilities() {
        let now = catalog_projection_now();
        let merged = merge_with_bundled_catalog(&[], &[], now, &CatalogFreshnessPolicy::default())
            .expect("bundled catalog");
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
            .expect("built-in template");
        let baseline = merged
            .models
            .iter()
            .find(|model| model.entry.provider_template_id == template.id)
            .expect("baseline model")
            .clone();
        let route = catalog_projection_route(now);

        let fresh_provenance = lorepia_providers::catalog::CatalogFieldProvenance {
            authority: CatalogAuthority::SignedCatalog,
            layer_id: "signed:test:2".to_owned(),
            revision: 2,
            verified_at: now - chrono::Duration::days(1),
            freshness: CatalogFreshness::Current,
        };
        let mut fresh = baseline.clone();
        fresh.entry.id = "test:exact:fresh".to_owned();
        fresh.entry.model_match = ModelMatch::Exact {
            model_id: route.model_id.clone(),
        };
        fresh.entry.metadata_version = 2;
        fresh.entry.verified_at = fresh_provenance.verified_at;
        fresh.entry.expires_at = Some(now + chrono::Duration::days(30));
        fresh.metadata_provenance = fresh_provenance.clone();
        let parameter_id = fresh.entry.parameters[0].id.clone();
        fresh.entry.parameters[0].label_key = "catalog.fresh.parameter".to_owned();
        fresh
            .parameter_provenance
            .insert(parameter_id.as_str().to_owned(), fresh_provenance.clone());
        let reasoning = fresh
            .entry
            .capabilities
            .iter_mut()
            .find(|capability| capability.key == CatalogCapabilityKey::Reasoning)
            .expect("reasoning capability");
        reasoning.value = CatalogCapabilityValue::Boolean(true);
        fresh
            .capability_provenance
            .insert(CatalogCapabilityKey::Reasoning, fresh_provenance);

        let stale_provenance = lorepia_providers::catalog::CatalogFieldProvenance {
            authority: CatalogAuthority::SignedCatalog,
            layer_id: "signed:test:3".to_owned(),
            revision: 3,
            verified_at: now - chrono::Duration::days(100),
            freshness: CatalogFreshness::Stale,
        };
        let mut stale = fresh.clone();
        stale.entry.id = "test:exact:stale".to_owned();
        stale.entry.metadata_version = 3;
        stale.entry.verified_at = stale_provenance.verified_at;
        stale.entry.parameters[0].label_key = "catalog.stale.parameter".to_owned();
        stale.metadata_provenance = stale_provenance.clone();
        stale
            .parameter_provenance
            .insert(parameter_id.as_str().to_owned(), stale_provenance.clone());
        stale
            .entry
            .capabilities
            .iter_mut()
            .find(|capability| capability.key == CatalogCapabilityKey::Reasoning)
            .expect("reasoning capability")
            .value = CatalogCapabilityValue::Boolean(false);
        stale
            .capability_provenance
            .insert(CatalogCapabilityKey::Reasoning, stale_provenance);

        let signed_layer_expirations = BTreeMap::from([
            ("signed:test:2".to_owned(), now + chrono::Duration::days(20)),
            ("signed:test:3".to_owned(), now + chrono::Duration::days(10)),
        ]);
        let projection = project_catalog_route(
            9,
            &[baseline, fresh, stale],
            &signed_layer_expirations,
            &route,
            &template.id,
        );
        assert!(projection.matched);
        assert_eq!(
            projection
                .parameters
                .iter()
                .find(|parameter| parameter.id == parameter_id)
                .expect("effective parameter")
                .label_key,
            "catalog.fresh.parameter"
        );
        assert_eq!(
            projection
                .signed_parameters
                .iter()
                .map(|parameter| parameter.label_key.as_str())
                .collect::<Vec<_>>(),
            vec!["catalog.fresh.parameter"],
            "a later stale signed value must not displace the current exact signed contract"
        );
        let reasoning = projection
            .capability_observations
            .iter()
            .find(|observation| observation.key == CapabilityKey::Reasoning)
            .expect("catalog capability observation");
        assert_eq!(reasoning.source, ObservationSource::SignedLorepiaCatalog);
        assert_eq!(reasoning.value, CapabilityValue::Boolean(false));
        assert_eq!(reasoning.status, SupportStatus::Unsupported);
        assert!(!reasoning.is_fresh_at(now));
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the test keeps AnyModel, glob, and later non-signed precedence in one projection matrix"
    )]
    fn route_projection_signed_parameters_require_current_specific_provenance() {
        let now = catalog_projection_now();
        let merged = merge_with_bundled_catalog(&[], &[], now, &CatalogFreshnessPolicy::default())
            .expect("bundled catalog");
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
            .expect("built-in template");
        let baseline = merged
            .models
            .iter()
            .find(|model| model.entry.provider_template_id == template.id)
            .expect("baseline model")
            .clone();
        let route = catalog_projection_route(now);
        let parameter_id = baseline.entry.parameters[0].id.clone();
        let bundled_projection = project_catalog_route(
            10,
            std::slice::from_ref(&baseline),
            &BTreeMap::new(),
            &route,
            &template.id,
        );
        assert!(bundled_projection.matched);
        assert!(
            bundled_projection.signed_parameters.is_empty(),
            "the bundled AnyModel baseline is not independent model-specific evidence"
        );
        let signed_provenance = lorepia_providers::catalog::CatalogFieldProvenance {
            authority: CatalogAuthority::SignedCatalog,
            layer_id: "signed:test:specific".to_owned(),
            revision: 4,
            verified_at: now - chrono::Duration::hours(1),
            freshness: CatalogFreshness::Current,
        };
        let signed_layer_expirations = BTreeMap::from([(
            signed_provenance.layer_id.clone(),
            now + chrono::Duration::days(30),
        )]);

        let mut signed_any = baseline.clone();
        signed_any.entry.id = "test:signed:any".to_owned();
        signed_any.entry.metadata_version = 4;
        signed_any.entry.verified_at = signed_provenance.verified_at;
        signed_any.entry.expires_at = Some(now + chrono::Duration::days(30));
        signed_any.entry.parameters[0].label_key = "catalog.signed.any".to_owned();
        signed_any.metadata_provenance = signed_provenance.clone();
        signed_any
            .parameter_provenance
            .insert(parameter_id.as_str().to_owned(), signed_provenance.clone());
        let any_projection = project_catalog_route(
            10,
            std::slice::from_ref(&signed_any),
            &signed_layer_expirations,
            &route,
            &template.id,
        );
        assert!(any_projection.matched);
        assert!(
            any_projection.signed_parameters.is_empty(),
            "a signed AnyModel fallback is not model-specific enough to drive OpenRouter wire fields"
        );

        let mut signed_glob = signed_any;
        signed_glob.entry.id = "test:signed:glob".to_owned();
        signed_glob.entry.model_match = ModelMatch::Glob {
            pattern: "example-*".to_owned(),
        };
        signed_glob.entry.parameters[0].label_key = "catalog.signed.glob".to_owned();
        let glob_projection = project_catalog_route(
            10,
            std::slice::from_ref(&signed_glob),
            &signed_layer_expirations,
            &route,
            &template.id,
        );
        assert_eq!(
            glob_projection
                .signed_parameters
                .iter()
                .map(|parameter| parameter.label_key.as_str())
                .collect::<Vec<_>>(),
            vec!["catalog.signed.glob"],
            "a current signed glob may provide a model-specific fallback contract"
        );

        let mut later_bundled = signed_glob.clone();
        later_bundled.entry.id = "test:bundled:exact".to_owned();
        later_bundled.entry.model_match = ModelMatch::Exact {
            model_id: route.model_id.clone(),
        };
        later_bundled.entry.metadata_version = 5;
        later_bundled.entry.parameters[0].label_key = "catalog.bundled.exact".to_owned();
        let bundled_provenance = lorepia_providers::catalog::CatalogFieldProvenance {
            authority: CatalogAuthority::Bundled,
            layer_id: "bundled:test:exact".to_owned(),
            revision: 5,
            verified_at: now,
            freshness: CatalogFreshness::Current,
        };
        later_bundled.metadata_provenance = bundled_provenance.clone();
        later_bundled
            .parameter_provenance
            .insert(parameter_id.as_str().to_owned(), bundled_provenance);
        let shadowed_projection = project_catalog_route(
            10,
            &[signed_glob, later_bundled],
            &signed_layer_expirations,
            &route,
            &template.id,
        );
        assert_eq!(
            shadowed_projection
                .parameters
                .iter()
                .find(|parameter| parameter.id == parameter_id)
                .expect("later bundled effective parameter")
                .label_key,
            "catalog.bundled.exact"
        );
        assert!(
            shadowed_projection.signed_parameters.is_empty(),
            "later selected non-signed provenance must remove a stale signed fallback candidate"
        );
    }
}
