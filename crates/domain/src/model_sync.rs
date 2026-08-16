use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    CapabilityObservation, CoreError, GenerationPreset, ModelRoute, ModelRouteId, ModelSyncJobId,
    ProviderConnection, ProviderConnectionId,
};

/// Current durable wire format for model synchronization progress events.
pub const MODEL_SYNC_EVENT_VERSION: u32 = 1;
pub const MODEL_SYNC_REDACTION_VERSION: u32 = 1;

/// Durable model synchronization state.
///
/// Network work is allowed only while a live Core process owns a job in
/// `Fetching`. Opening a database converts abandoned network/commit states to
/// `Interrupted`; it never retries provider requests automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelSyncState {
    #[serde(rename = "created")]
    Created,
    #[serde(rename = "fetching")]
    Fetching,
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "diff-ready-awaiting-review")]
    DiffReadyAwaitingReview,
    #[serde(rename = "committing")]
    Committing,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}

impl ModelSyncState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Non-secret source of a provider model-list observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncSourceProvenance {
    pub source: String,
    pub api_family: crate::ApiFamily,
    pub api_origin: crate::CanonicalOrigin,
    pub endpoint_path: crate::EndpointPath,
    pub pages_fetched: u32,
    pub response_bytes: u64,
}

/// Exact, canonical commit plan shown to a user before approval.
///
/// The expected connection snapshot contains only public configuration and an
/// opaque credential reference. Credential material is request-scoped and can
/// never be represented by this type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncDiff {
    pub connection_id: ProviderConnectionId,
    pub expected_connection: ProviderConnection,
    /// Exact route graph observed before the network request. Approval fails
    /// closed if another writer changes this graph before commit.
    pub expected_model_routes: Vec<ModelRoute>,
    pub observed_at: DateTime<Utc>,
    pub listed_routes: Vec<ModelRoute>,
    pub newly_seen_model_route_ids: Vec<ModelRouteId>,
    pub missing_model_route_ids: Vec<ModelRouteId>,
    pub initial_presets: Vec<GenerationPreset>,
    pub capability_observations: Vec<CapabilityObservation>,
    pub routes_requiring_preset_configuration: Vec<ModelRouteId>,
    pub provenance: ModelSyncSourceProvenance,
}

impl ModelSyncDiff {
    /// Sorts every set-like collection before hashing or persistence.
    pub fn canonicalize(&mut self) {
        self.listed_routes.sort_by(|left, right| {
            left.model_id
                .cmp(&right.model_id)
                .then_with(|| left.id.cmp(&right.id))
        });
        self.expected_model_routes.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.model_id.cmp(&right.model_id))
        });
        self.newly_seen_model_route_ids.sort();
        self.newly_seen_model_route_ids.dedup();
        self.missing_model_route_ids.sort();
        self.missing_model_route_ids.dedup();
        self.initial_presets
            .sort_by(|left, right| left.id.cmp(&right.id));
        self.capability_observations
            .sort_by(|left, right| left.id.cmp(&right.id));
        self.routes_requiring_preset_configuration.sort();
        self.routes_requiring_preset_configuration.dedup();
    }

    pub fn canonical_json(&self) -> Result<Vec<u8>, String> {
        let mut canonical = self.clone();
        canonical.canonicalize();
        serde_json::to_vec(&canonical)
            .map_err(|error| format!("cannot encode model synchronization review: {error}"))
    }

    pub fn review_sha256(&self) -> Result<String, String> {
        Ok(hex::encode(Sha256::digest(self.canonical_json()?)))
    }
}

/// User-reviewable synchronization proposal and its canonical digest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncReview {
    pub sha256: String,
    pub diff: ModelSyncDiff,
}

impl ModelSyncReview {
    pub fn new(mut diff: ModelSyncDiff) -> Result<Self, String> {
        diff.canonicalize();
        let sha256 = diff.review_sha256()?;
        Ok(Self { sha256, diff })
    }

    /// Recomputes the digest instead of trusting the stored digest field.
    pub fn verify(&self) -> Result<(), String> {
        let recomputed = self.diff.review_sha256()?;
        if recomputed != self.sha256 {
            return Err("model synchronization review hash does not match its diff".to_owned());
        }
        Ok(())
    }
}

/// Deliberately bounded, provider-detail-free failure information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncFailure {
    pub code: String,
    pub message_key: String,
    pub recoverable: bool,
}

impl ModelSyncFailure {
    pub fn from_core_error(error: &CoreError) -> Self {
        Self {
            code: error.code.as_str().to_owned(),
            message_key: "model_sync.failed".to_owned(),
            recoverable: error.recoverable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncProgress {
    pub completed_steps: u32,
    pub total_steps: u32,
    pub message_key: String,
}

/// One versioned event in the durable model-sync outbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncEvent {
    pub version: u32,
    pub job_id: ModelSyncJobId,
    pub sequence: u64,
    pub job_revision: u64,
    pub redaction_version: u32,
    pub state: ModelSyncState,
    pub progress: ModelSyncProgress,
    pub review_sha256: Option<String>,
    pub failure: Option<ModelSyncFailure>,
    pub emitted_at: DateTime<Utc>,
}

/// Durable job snapshot returned by Core.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncJob {
    pub id: ModelSyncJobId,
    pub connection_id: ProviderConnectionId,
    pub state: ModelSyncState,
    pub revision: u64,
    pub review: Option<ModelSyncReview>,
    pub failure: Option<ModelSyncFailure>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{MODEL_SYNC_EVENT_VERSION, ModelSyncState};

    #[test]
    fn state_wire_names_and_event_version_are_stable() {
        assert_eq!(MODEL_SYNC_EVENT_VERSION, 1);
        assert_eq!(
            serde_json::to_string(&ModelSyncState::DiffReadyAwaitingReview)
                .expect("serialize model-sync state"),
            "\"diff-ready-awaiting-review\""
        );
        assert!(ModelSyncState::Completed.is_terminal());
        assert!(!ModelSyncState::Interrupted.is_terminal());
        let _ = Utc
            .with_ymd_and_hms(2026, 7, 31, 12, 0, 0)
            .single()
            .expect("valid test timestamp");
    }
}
