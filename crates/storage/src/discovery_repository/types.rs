//! Discovery repository input and result types.

use chrono::{DateTime, Utc};
use lorepia_domain::{
    CapabilityObservation, CoreError, CoreResult, DiscoverySessionId, EvidenceId, GenerationPreset,
    HttpUrl, ModelRoute, ProviderConnection, ProviderConnectionId, ProviderTemplate,
    discovery::{
        DiscoveryActionId, DiscoveryActionReceipt, DiscoveryApprovalBinding,
        DiscoveryApprovalRecord, DiscoveryCommitAttemptId,
        DiscoveryCommitPhase as DomainCommitPhase, DiscoveryCommitPlan, DiscoveryCompensationKind,
        DiscoveryCompensationStatus as DomainCompensationStatus, DiscoveryCompensationStep,
        DiscoveryOperationId, DiscoveryOperationKind, DiscoveryReviewDiff,
        DiscoverySideEffectClass, DiscoveryState, DiscoveryTransition, ProviderDiscoveryEvent,
        ProviderDiscoverySession,
    },
};
use serde_json::Value;

use crate::discovery::DurableOperationOutcome;

use super::{
    errors::corrupted, native_no_effect_evidence_sha256, provider_graph_ownership_hash,
    repository_io::validate_discovery_native_physical_authority_id,
    semantic_view::StoredDiscoveryCandidate,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverySessionSnapshot {
    pub session: ProviderDiscoverySession,
    pub active_operation_id: Option<DiscoveryOperationId>,
    pub draft_json: Option<Value>,
    pub review: Option<DiscoveryReviewDiff>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryJsonUpdate<T> {
    Preserve,
    Clear,
    Replace(T),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryEvidenceKind {
    HtmlDocument,
    JsonDocument,
    YamlDocument,
    XmlDocument,
    PlainTextDocument,
    JsonSchema,
    OpenApi,
}

impl DiscoveryEvidenceKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::HtmlDocument => "html_document",
            Self::JsonDocument => "json_document",
            Self::YamlDocument => "yaml_document",
            Self::XmlDocument => "xml_document",
            Self::PlainTextDocument => "plain_text_document",
            Self::JsonSchema => "json_schema",
            Self::OpenApi => "open_api",
        }
    }

    pub(super) fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "html_document" => Ok(Self::HtmlDocument),
            "json_document" => Ok(Self::JsonDocument),
            "yaml_document" => Ok(Self::YamlDocument),
            "xml_document" => Ok(Self::XmlDocument),
            "plain_text_document" => Ok(Self::PlainTextDocument),
            "json_schema" => Ok(Self::JsonSchema),
            "open_api" => Ok(Self::OpenApi),
            _ => Err(corrupted("stored discovery evidence kind is invalid")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryEvidenceRecord {
    pub id: EvidenceId,
    pub session_id: DiscoverySessionId,
    pub kind: DiscoveryEvidenceKind,
    pub source_url: HttpUrl,
    pub content_sha256: String,
    pub extracted_json: Value,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryOperationStatus {
    Prepared,
    Started,
    Succeeded,
    Failed,
    Interrupted,
    OutcomeUnknown,
}

impl DiscoveryOperationStatus {
    pub(super) fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "started" => Ok(Self::Started),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            "outcome_unknown" => Ok(Self::OutcomeUnknown),
            _ => Err(corrupted("stored discovery operation status is invalid")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryOperationRecord {
    pub id: DiscoveryOperationId,
    pub session_id: DiscoverySessionId,
    pub kind: DiscoveryOperationKind,
    pub side_effect_class: DiscoverySideEffectClass,
    pub status: DiscoveryOperationStatus,
    pub action_id: DiscoveryActionId,
    pub expected_revision: u64,
    pub request_sha256: String,
    pub approval: Option<DiscoveryApprovalBinding>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Immutable binding between one semantic discovery operation and the fresh
/// physical native credential authority reserved before that operation starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryNativeCredentialExecutionRecord {
    pub physical_authority_id: String,
    pub operation_id: DiscoveryOperationId,
    pub session_id: DiscoverySessionId,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub commit_plan_sha256: String,
    pub connection_id: ProviderConnectionId,
    pub connection_binding_sha256: String,
    pub reserved_at: DateTime<Utc>,
    pub store_started_at: Option<DateTime<Utc>>,
}

/// Exact inputs Core has revalidated before reserving a fresh physical slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryNativeCredentialExecutionReservation {
    pub operation_id: DiscoveryOperationId,
    pub session_id: DiscoverySessionId,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub commit_plan_sha256: String,
    pub connection_id: ProviderConnectionId,
    pub connection_binding_sha256: String,
    pub reserved_at: DateTime<Utc>,
}

/// Exact reserved execution whose next native action is the credential store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryNativeCredentialStoreAttemptStart {
    pub operation_id: DiscoveryOperationId,
    pub physical_authority_id: String,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryOutboxEvent {
    pub event: ProviderDiscoveryEvent,
    pub delivery_attempts: u32,
    pub available_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryCommitPhase {
    Prepared,
    DatabaseApplied,
    CredentialReferenceApplied,
    Completed,
    CompensationRequired,
    Compensating,
    Compensated,
    OutcomeUnknown,
}

impl DiscoveryCommitPhase {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::DatabaseApplied => "database_applied",
            Self::CredentialReferenceApplied => "credential_reference_applied",
            Self::Completed => "completed",
            Self::CompensationRequired => "compensation_required",
            Self::Compensating => "compensating",
            Self::Compensated => "compensated",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }

    pub(super) fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "database_applied" => Ok(Self::DatabaseApplied),
            "credential_reference_applied" => Ok(Self::CredentialReferenceApplied),
            "completed" => Ok(Self::Completed),
            "compensation_required" => Ok(Self::CompensationRequired),
            "compensating" => Ok(Self::Compensating),
            "compensated" => Ok(Self::Compensated),
            "outcome_unknown" => Ok(Self::OutcomeUnknown),
            _ => Err(corrupted("stored discovery commit phase is invalid")),
        }
    }
}

impl From<DomainCommitPhase> for DiscoveryCommitPhase {
    fn from(value: DomainCommitPhase) -> Self {
        match value {
            DomainCommitPhase::Prepared => Self::Prepared,
            DomainCommitPhase::DatabaseApplied => Self::DatabaseApplied,
            DomainCommitPhase::CredentialReferenceApplied => Self::CredentialReferenceApplied,
            DomainCommitPhase::Completed => Self::Completed,
            DomainCommitPhase::CompensationRequired => Self::CompensationRequired,
            DomainCommitPhase::Compensating => Self::Compensating,
            DomainCommitPhase::Compensated => Self::Compensated,
            DomainCommitPhase::OutcomeUnknown => Self::OutcomeUnknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryCommitAttemptRecord {
    pub id: DiscoveryCommitAttemptId,
    pub session_id: DiscoverySessionId,
    pub attempt_number: u32,
    pub action_id: DiscoveryActionId,
    pub expected_revision: u64,
    pub plan_sha256: String,
    pub plan: DiscoveryCommitPlan,
    pub phase: DiscoveryCommitPhase,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryCompensationStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    OutcomeUnknown,
}

impl DiscoveryCompensationStatus {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }

    pub(super) fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "outcome_unknown" => Ok(Self::OutcomeUnknown),
            _ => Err(corrupted("stored discovery compensation status is invalid")),
        }
    }
}

impl From<DomainCompensationStatus> for DiscoveryCompensationStatus {
    fn from(value: DomainCompensationStatus) -> Self {
        match value {
            DomainCompensationStatus::Pending => Self::Pending,
            DomainCompensationStatus::InProgress => Self::InProgress,
            DomainCompensationStatus::Completed => Self::Completed,
            DomainCompensationStatus::Failed => Self::Failed,
            DomainCompensationStatus::OutcomeUnknown => Self::OutcomeUnknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryCompensationRecord {
    pub id: String,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub ordinal: u32,
    pub action_id: DiscoveryActionId,
    pub kind: DiscoveryCompensationKind,
    pub step: DiscoveryCompensationStep,
    pub status: DiscoveryCompensationStatus,
    pub attempt_count: u32,
    pub last_failure: Option<lorepia_domain::discovery::DiscoveryFailure>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedDiscoveryCompensationStep {
    pub id: String,
    pub step: DiscoveryCompensationStep,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedDiscoveryCommit {
    pub plan: DiscoveryCommitPlan,
    pub plan_sha256: String,
    pub attempt_number: u32,
    pub reuse_existing: bool,
    pub compensation_steps: Vec<PreparedDiscoveryCompensationStep>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredProviderGraph {
    pub plan: DiscoveryCommitPlan,
    pub plan_sha256: String,
    pub template: ProviderTemplate,
    pub connection: ProviderConnection,
    pub routes: Vec<ModelRoute>,
    pub observations: Vec<CapabilityObservation>,
    pub presets: Vec<GenerationPreset>,
}

impl DiscoveredProviderGraph {
    pub fn ownership_sha256(&self) -> CoreResult<String> {
        provider_graph_ownership_hash(
            &self.template,
            &self.connection,
            &self.routes,
            &self.observations,
            &self.presets,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryCompletedOperationWrite {
    pub id: DiscoveryOperationId,
    pub outcome: DurableOperationOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryNativeNoEffectAttestationKind {
    CredentialSlotMissing,
}

impl DiscoveryNativeNoEffectAttestationKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::CredentialSlotMissing => "credential_slot_missing",
        }
    }

    pub(super) fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "credential_slot_missing" => Ok(Self::CredentialSlotMissing),
            _ => Err(corrupted(
                "stored native no-effect attestation kind is invalid",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryNativeRecoveryOwner {
    NativePlatform,
}

impl DiscoveryNativeRecoveryOwner {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::NativePlatform => "native_platform",
        }
    }

    pub(super) fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "native_platform" => Ok(Self::NativePlatform),
            _ => Err(corrupted(
                "stored native no-effect attestation owner is invalid",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryNativeNoEffectAttestationWrite {
    pub operation_id: DiscoveryOperationId,
    pub physical_authority_id: String,
    pub session_id: DiscoverySessionId,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub commit_plan_sha256: String,
    pub connection_id: ProviderConnectionId,
    pub kind: DiscoveryNativeNoEffectAttestationKind,
    pub recovery_owner: DiscoveryNativeRecoveryOwner,
    pub evidence_sha256: String,
}

impl DiscoveryNativeNoEffectAttestationWrite {
    pub fn credential_slot_missing(
        operation_id: DiscoveryOperationId,
        physical_authority_id: String,
        session_id: DiscoverySessionId,
        commit_attempt_id: DiscoveryCommitAttemptId,
        commit_plan_sha256: String,
        connection_id: ProviderConnectionId,
    ) -> CoreResult<Self> {
        validate_discovery_native_physical_authority_id(&physical_authority_id)
            .map_err(|_| CoreError::invalid("native no-effect physical authority is invalid"))?;
        let mut attestation = Self {
            operation_id,
            physical_authority_id,
            session_id,
            commit_attempt_id,
            commit_plan_sha256,
            connection_id,
            kind: DiscoveryNativeNoEffectAttestationKind::CredentialSlotMissing,
            recovery_owner: DiscoveryNativeRecoveryOwner::NativePlatform,
            evidence_sha256: String::new(),
        };
        attestation.evidence_sha256 = native_no_effect_evidence_sha256(&attestation)?;
        Ok(attestation)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryNativeNoEffectAttestationRecord {
    pub operation_id: DiscoveryOperationId,
    pub physical_authority_id: String,
    pub session_id: DiscoverySessionId,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub commit_plan_sha256: String,
    pub connection_id: ProviderConnectionId,
    pub kind: DiscoveryNativeNoEffectAttestationKind,
    pub recovery_owner: DiscoveryNativeRecoveryOwner,
    pub evidence_sha256: String,
    pub connection_binding_sha256: String,
    pub execution_binding_sha256: String,
    pub attested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryTransitionWrite {
    pub transition: DiscoveryTransition,
    pub draft: DiscoveryJsonUpdate<Value>,
    pub review: DiscoveryJsonUpdate<DiscoveryReviewDiff>,
    pub new_evidence: Vec<DiscoveryEvidenceRecord>,
    pub new_candidates: Vec<StoredDiscoveryCandidate>,
    pub approval: Option<DiscoveryApprovalRecord>,
    pub new_operation_id: Option<DiscoveryOperationId>,
    pub completed_operation: Option<DiscoveryCompletedOperationWrite>,
    pub prepared_commit: Option<PreparedDiscoveryCommit>,
    pub provider_graph: Option<DiscoveredProviderGraph>,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRecoveryResult {
    pub operation_id: DiscoveryOperationId,
    pub session_id: DiscoverySessionId,
    pub state: DiscoveryState,
    pub event: ProviderDiscoveryEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryActionReplay {
    pub receipt: DiscoveryActionReceipt,
    pub transition: DiscoveryTransition,
}
