//! Durable provider-discovery orchestration.
//!
//! This module is deliberately synchronous at the Core boundary. Network work
//! is executed on Core's owned Tokio runtime only after the corresponding
//! operation, action receipt, audit entry, and outbox event have been prepared
//! in `SQLite`. Raw credentials are borrowed by one request and never enter the
//! working draft, an action, an operation, evidence, an error, or an event.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ApiFamily, AuthBinding, CanonicalOrigin, CapabilityObservation, CapabilityValue,
    ConnectionConfig, ConnectionConfigValue, ConnectionFieldType, ConnectionStatus, ConversationId,
    CoreError, CoreErrorCode, CoreResult, CredentialRedirectPolicy, CredentialRef, CredentialScope,
    DecoderId, DiscoverySessionId, EvidenceId, GenerationId, GenerationPreset, GenerationRequest,
    GenerationTarget, HttpMethod, HttpUrl, Message, MessageRole, ModelRoute, ModelRouteId,
    ProviderConnection, ProviderConnectionId, ProviderLocalNetworkApproval, ProviderManifest,
    ProviderNetworkMode, ProviderTemplate, ProviderTemplateId, SupportStatus, TemplateSource,
    discovery::{
        DiscoveryActionEnvelope, DiscoveryActionId, DiscoveryApprovalBinding,
        DiscoveryApprovalDecision, DiscoveryApprovalGrant, DiscoveryApprovalId,
        DiscoveryApprovalRecord, DiscoveryAssistantCheckpoint, DiscoveryCandidate,
        DiscoveryCandidateId, DiscoveryCandidateSummary, DiscoveryCatalogAuthorityBinding,
        DiscoveryCommitAttemptId, DiscoveryCommitPlan, DiscoveryCompensationKind,
        DiscoveryCompensationStatus, DiscoveryCompensationStep, DiscoveryCompensationTarget,
        DiscoveryEffect, DiscoveryEventId, DiscoveryEvidenceResolution, DiscoveryFailure,
        DiscoveryFreshEvidenceSource, DiscoveryInterruptionOutcome, DiscoveryOperationId,
        DiscoveryOperationKind, DiscoveryPreviousSelection, DiscoveryProbeBudget,
        DiscoveryReviewChange, DiscoveryReviewChangeKind, DiscoveryReviewDiff, DiscoveryState,
        ProviderDiscoveryAction, ProviderDiscoveryConnectionOptions, ProviderDiscoverySession,
        SanitizedDiscoveryInput,
    },
};
use lorepia_providers::{
    AdapterRegistry, BuiltInTemplateId, CapabilityProbeEngine, CapabilityProbeKind, CurlAuthHint,
    ModelListRequest, ParsedCurlEvidence, ProbeBudget, ProbeConsent, ProbeRunOutcome, Provider,
    ProviderCapabilityProbeAdapter, ProviderEvent, RequestPreview, SecretBytes, SecretCurlInput,
    discovery::DiscoveryFetchBudget,
    inspect_curl,
    setup_assistant::{
        AssistantBudget, AssistantCallEstimate, AssistantConsent, AssistantDraftReview,
        AssistantEngineSnapshot, AssistantError, AssistantEvidenceKind, AssistantFailureKind,
        AssistantHostAction, AssistantPromptPackage, AssistantState, AssistantToolCall,
        AssistantToolResult, DraftField, EvidenceClaim, RedactedAssistantEvidence,
        SetupAssistantEngine, UnresolvedQuestion,
    },
    url_policy::{ApprovedLocalNetworkOrigin, UrlPolicy},
    validate_connection_fields, validate_manifest,
};
use lorepia_storage::{
    DiscoveredProviderGraph, DiscoveryCandidateSnapshot as CandidateView, DiscoveryCommitPhase,
    DiscoveryCompletedOperationWrite, DiscoveryEvidenceKind, DiscoveryEvidenceRecord,
    DiscoveryJsonUpdate, DiscoveryNativeCredentialExecutionRecord,
    DiscoveryNativeCredentialExecutionReservation, DiscoveryNativeCredentialStoreAttemptStart,
    DiscoveryNativeNoEffectAttestationWrite, DiscoveryOperationStatus, DiscoveryOutboxEvent,
    DiscoveryRecoveryResult, DiscoverySessionSnapshot, DiscoveryTransitionWrite,
    DurableOperationOutcome, PreparedDiscoveryCommit, PreparedDiscoveryCompensationStep,
    ProviderCredentialAccessAuthority, Storage, StoredDiscoveryCandidate,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{
    runtime::Handle,
    sync::{mpsc, watch},
};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    DiscoveryRecoveryOwner,
    app::{
        initial_generation_preset, provider_api_capability_observations, reconcile_input_routes,
        template_accepts_empty_preset,
    },
    catalog::operational_provider_catalog_projection_for_storage,
    provider_discovery_deterministic::{
        DeterministicDiscoveryErrorKind, DeterministicDiscoveryExecutor,
        DeterministicDiscoveryOutput, DeterministicDiscoverySource, DiscoveryCandidateConfidence,
        embed_discovered_api_base_path,
    },
};

const WORKING_DRAFT_SCHEMA_VERSION: u32 = 1;
const MAX_DISCOVERY_ROWS: u32 = 1_000;
const MAX_AUTOMATIC_EFFECTS: usize = 16;
const MAX_ASSISTANT_HOST_STEPS: usize = 32;
const DISCOVERY_NAMESPACE: Uuid = Uuid::from_u128(0x9098_a11c_20bb_4d28_a758_8a17_efc8_0882);

/// Exact durable context authorizing one native credential installation.
///
/// This Rust-only value contains opaque identifiers and hashes, never
/// credential material. It binds the vault slot to one approved discovery
/// commit attempt and its currently active operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCredentialInstallContext {
    pub session_id: DiscoverySessionId,
    pub session_revision: u64,
    pub operation_id: DiscoveryOperationId,
    pub operation_status: DiscoveryOperationStatus,
    /// Durable pre-store reservation. This may be present while the semantic
    /// operation remains `Prepared`, but it is not yet authority to write or
    /// confirm the native slot.
    pub native_execution_reservation_id: Option<String>,
    /// Physical native-slot authority after the exact reservation is sealed to
    /// the durable `Started` transition. A prepared operation always has no
    /// value here, including after reservation.
    pub native_execution_id: Option<String>,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub commit_plan_sha256: String,
    pub commit_phase: DiscoveryCommitPhase,
    pub connection_id: ProviderConnectionId,
    pub connection_binding_sha256: String,
}

/// Exact native proof that the current operation's authority-scoped slot was
/// observed after its durable start marker.
///
/// This Rust-only value contains no credential material. Keeping the operation
/// and commit-attempt identities separate prevents a prior retry's physical
/// slot from being adopted by a later operation which reuses the same
/// immutable commit plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCredentialCommitConfirmation {
    pub operation_id: DiscoveryOperationId,
    pub native_execution_id: String,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub commit_plan_sha256: String,
    pub connection_id: ProviderConnectionId,
    pub connection_binding_sha256: String,
}

impl TryFrom<&ProviderDiscoveryCredentialInstallContext>
    for ProviderDiscoveryCredentialCommitConfirmation
{
    type Error = CoreError;

    fn try_from(value: &ProviderDiscoveryCredentialInstallContext) -> CoreResult<Self> {
        if value.operation_status != DiscoveryOperationStatus::Started
            || value.commit_phase != DiscoveryCommitPhase::Prepared
        {
            return Err(CoreError::invalid(
                "native credential confirmation requires a started commit operation",
            ));
        }
        let native_execution_id = value.native_execution_id.clone().ok_or_else(|| {
            CoreError::invalid(
                "native credential confirmation requires a started execution incarnation",
            )
        })?;
        if value.native_execution_reservation_id.as_deref() != Some(native_execution_id.as_str()) {
            return Err(CoreError::invalid(
                "native credential confirmation differs from its reserved execution incarnation",
            ));
        }
        Ok(Self {
            operation_id: value.operation_id.clone(),
            native_execution_id,
            commit_attempt_id: value.commit_attempt_id.clone(),
            commit_plan_sha256: value.commit_plan_sha256.clone(),
            connection_id: value.connection_id.clone(),
            connection_binding_sha256: value.connection_binding_sha256.clone(),
        })
    }
}

fn native_credential_execution_context_ids(
    operation_status: DiscoveryOperationStatus,
    operation_started_at: Option<&DateTime<Utc>>,
    native_execution: Option<DiscoveryNativeCredentialExecutionRecord>,
    recovery_context: bool,
) -> CoreResult<(Option<String>, Option<String>)> {
    match (operation_status, native_execution) {
        (DiscoveryOperationStatus::Prepared, None) if operation_started_at.is_none() => {
            Ok((None, None))
        }
        (DiscoveryOperationStatus::Prepared, Some(execution))
            if operation_started_at.is_none() && execution.store_started_at.is_none() =>
        {
            Ok((Some(execution.physical_authority_id), None))
        }
        (DiscoveryOperationStatus::Started, Some(execution))
            if execution.store_started_at.is_some()
                && operation_started_at == execution.store_started_at.as_ref() =>
        {
            let physical_authority_id = execution.physical_authority_id;
            Ok((
                Some(physical_authority_id.clone()),
                Some(physical_authority_id),
            ))
        }
        (DiscoveryOperationStatus::Started, None)
            if recovery_context && operation_started_at.is_some() =>
        {
            // Storage returns no execution only for the immutable schema-37
            // cutoff snapshot of an already-Started legacy lineage. It has no
            // physical authority and is exposed solely so startup can classify
            // the semantic operation as outcome-unknown instead of
            // synthesizing or adopting a B.
            Ok((None, None))
        }
        (DiscoveryOperationStatus::Started, None) => Err(CoreError::invalid(
            "started credential installation has no native execution authority",
        )),
        _ => Err(CoreError::invalid(
            "native credential reservation and store attempt are inconsistent",
        )),
    }
}

/// Stable pre-commit authority for one discovery-scoped native credential.
///
/// This Rust-only value never contains credential material. The approval ID
/// and grant hash name the exact credential-origin approval, while the
/// connection hash binds the eventual provider credential scope before the
/// provider graph is published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCredentialLeaseContext {
    pub session_id: DiscoverySessionId,
    pub connection_id: ProviderConnectionId,
    pub credential_api_origin: CanonicalOrigin,
    pub credential_origin_approval_id: DiscoveryApprovalId,
    pub credential_origin_grant_sha256: String,
    pub connection_binding_sha256: String,
}

/// Exact secure-item authority for a compensating discovery removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCredentialAuthority {
    pub operation_id: DiscoveryOperationId,
    pub native_execution_id: String,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub connection_id: ProviderConnectionId,
    pub credential_api_origin: CanonicalOrigin,
    pub credential_origin_approval_id: DiscoveryApprovalId,
    pub credential_origin_grant_sha256: String,
    pub connection_binding_sha256: String,
}

/// One immutable approval proposal derived from the current durable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryApprovalProposal {
    pub id: DiscoveryApprovalId,
    pub grant: DiscoveryApprovalGrant,
    pub grant_sha256: String,
}

/// Review data plus the exact commit values that the approval action must echo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryReviewProposal {
    pub review: DiscoveryReviewDiff,
    pub approval: ProviderDiscoveryApprovalProposal,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub commit_plan_sha256: String,
    pub request_preview: Option<RequestPreview>,
}

/// One exact native action which can safely resume a durable setup-assistant
/// boundary. Native clients must not infer this from the overall discovery
/// state or from opaque draft JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderDiscoveryAssistantResumeAction {
    ApproveConsent,
    RunAssistant,
    WaitForAssistantOutcome,
    ResumeCoreHostAction,
    SupplyMoreEvidence,
    ApproveRetry,
    ReviewDraft,
    RestartInterrupted,
    ResolveUnknownOutcome,
}

/// Typed, secret-free recovery surface for a setup-assistant session.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderDiscoveryAssistantResumeBoundary {
    pub checkpoint: Option<DiscoveryAssistantCheckpoint>,
    pub action: ProviderDiscoveryAssistantResumeAction,
    pub questions: Vec<UnresolvedQuestion>,
    pub draft_review: Option<AssistantDraftReview>,
}

mod actions;
mod begin;
mod driver;
mod types;
mod views;

pub use actions::provider_discovery_action_envelope;
pub(crate) use types::ProviderDiscoveryOrchestrator;

#[cfg(test)]
use actions::write_canonical_json;
use actions::{canonical_sha256, sha256_hex};
use begin::{
    additional_curl_url_policy, additional_document_url_policy,
    credential_bearing_curl_requires_handoff, discovery_url_policy,
};
use driver::{
    EffectCompletion, hydrate_working_draft, operation_for_effect, transition_error,
    working_draft_value,
};
use types::{DiscoverySourceIntent, DiscoveryWorkingDraft};
pub use types::{
    ProviderCurlInspection, ProviderDiscoveryAdditionalEvidence, ProviderDiscoveryCurlInput,
    ProviderDiscoverySource,
};

impl ProviderDiscoverySource {
    pub fn known_provider(template: BuiltInTemplateId) -> Self {
        Self::known_provider_id(lorepia_domain::ProviderTemplateId::from(template.as_str()))
    }

    pub fn known_provider_id(template_id: lorepia_domain::ProviderTemplateId) -> Self {
        Self {
            intent: DiscoverySourceIntent::KnownProvider { template_id },
            transient: None,
            declared_connection_options: None,
            derived_site_url: None,
        }
    }
}

impl ProviderDiscoveryOrchestrator<'_> {
    pub fn assistant_resume_boundary(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<ProviderDiscoveryAssistantResumeBoundary>> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        match snapshot.session.state {
            DiscoveryState::AwaitingAssistantConsent => {
                let engine = restored_assistant(&draft)?;
                if engine.state() != AssistantState::AwaitingConsent {
                    return Err(corrupted_assistant_resume_boundary());
                }
                Ok(Some(ProviderDiscoveryAssistantResumeBoundary {
                    checkpoint: None,
                    action: ProviderDiscoveryAssistantResumeAction::ApproveConsent,
                    questions: Vec::new(),
                    draft_review: None,
                }))
            }
            DiscoveryState::BuildingAssistantManifestDraft => {
                let engine = restored_assistant(&draft)?;
                let checkpoint = assistant_checkpoint(engine.state())?;
                let action = match engine.state() {
                    AssistantState::Ready => ProviderDiscoveryAssistantResumeAction::RunAssistant,
                    AssistantState::AwaitingAssistant => {
                        ProviderDiscoveryAssistantResumeAction::WaitForAssistantOutcome
                    }
                    AssistantState::AwaitingToolResult => {
                        ProviderDiscoveryAssistantResumeAction::ResumeCoreHostAction
                    }
                    AssistantState::AwaitingRetryConsent => {
                        ProviderDiscoveryAssistantResumeAction::ApproveRetry
                    }
                    AssistantState::DraftReady => {
                        ProviderDiscoveryAssistantResumeAction::ReviewDraft
                    }
                    AssistantState::AwaitingMoreEvidence
                    | AssistantState::AwaitingConsent
                    | AssistantState::Interrupted
                    | AssistantState::Failed
                    | AssistantState::Cancelled => {
                        return Err(corrupted_assistant_resume_boundary());
                    }
                };
                let draft_review = if action == ProviderDiscoveryAssistantResumeAction::ReviewDraft
                {
                    Some(
                        engine
                            .draft_review()
                            .cloned()
                            .ok_or_else(corrupted_assistant_resume_boundary)?,
                    )
                } else {
                    None
                };
                Ok(Some(ProviderDiscoveryAssistantResumeBoundary {
                    checkpoint: Some(checkpoint),
                    action,
                    questions: Vec::new(),
                    draft_review,
                }))
            }
            DiscoveryState::AwaitingMoreEvidence if draft.assistant.is_some() => {
                let engine = restored_assistant(&draft)?;
                if engine.state() != AssistantState::AwaitingMoreEvidence
                    || draft.assistant_more_evidence_questions.is_empty()
                {
                    return Err(corrupted_assistant_resume_boundary());
                }
                Ok(Some(ProviderDiscoveryAssistantResumeBoundary {
                    checkpoint: Some(DiscoveryAssistantCheckpoint::AwaitingMoreEvidence),
                    action: ProviderDiscoveryAssistantResumeAction::SupplyMoreEvidence,
                    questions: draft.assistant_more_evidence_questions,
                    draft_review: None,
                }))
            }
            DiscoveryState::Interrupted
                if snapshot.session.recovery.as_ref().is_some_and(|recovery| {
                    recovery.operation == DiscoveryOperationKind::BuildAssistantManifestDraft
                }) =>
            {
                Ok(Some(ProviderDiscoveryAssistantResumeBoundary {
                    checkpoint: None,
                    action: ProviderDiscoveryAssistantResumeAction::RestartInterrupted,
                    questions: Vec::new(),
                    draft_review: None,
                }))
            }
            DiscoveryState::UnknownOutcome
                if snapshot.session.unknown_operation
                    == Some(DiscoveryOperationKind::BuildAssistantManifestDraft) =>
            {
                Ok(Some(ProviderDiscoveryAssistantResumeBoundary {
                    checkpoint: None,
                    action: ProviderDiscoveryAssistantResumeAction::ResolveUnknownOutcome,
                    questions: Vec::new(),
                    draft_review: None,
                }))
            }
            _ => Ok(None),
        }
    }

    /// Marks unfinished work interrupted or outcome-unknown. It never executes
    /// a prepared operation and therefore never replays a request on startup.
    pub fn recover_startup(
        &self,
        recovered_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryRecoveryResult>> {
        let resumable = resumable_assistant_operation_ids(self.storage)?;
        self.storage
            .recover_unfinished_discovery_operations_except(recovered_at, &resumable)
    }

    pub fn unfinished_recovery_candidates(&self) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        self.storage
            .list_unfinished_discovery_sessions_for_recovery()
    }

    pub fn credential_recovery_candidates(&self) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        self.unfinished_recovery_candidates()?
            .into_iter()
            .filter(|snapshot| {
                snapshot.session.state == DiscoveryState::Committing
                    && snapshot.session.input.credential_ref.is_some()
            })
            .map(|snapshot| {
                let operation = self
                    .storage
                    .get_current_discovery_operation(&snapshot.session.id)?
                    .ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::StorageCorrupted,
                            "credential recovery candidate has no active operation",
                            false,
                        )
                    })?;
                if operation.kind != DiscoveryOperationKind::AtomicCommit
                    || !matches!(
                        operation.status,
                        DiscoveryOperationStatus::Prepared | DiscoveryOperationStatus::Started
                    )
                    || snapshot.active_operation_id.as_ref() != Some(&operation.id)
                {
                    return Err(CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "credential recovery candidate is detached from its active operation",
                        false,
                    ));
                }
                Ok(snapshot)
            })
            .collect()
    }

    pub fn compensation_steps(
        &self,
        attempt_id: &DiscoveryCommitAttemptId,
    ) -> CoreResult<Vec<lorepia_storage::DiscoveryCompensationRecord>> {
        self.storage.list_discovery_compensation_steps(attempt_id)
    }

    /// Starts the compensation operation and executes only Core-owned database
    /// steps. It stops before native credential deletion and never retries a
    /// failed or unknown step.
    #[allow(clippy::too_many_lines)]
    pub fn continue_compensation(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::Compensating {
            return Err(CoreError::invalid("provider discovery is not compensating"));
        }
        if snapshot.session.failure.is_some() {
            return Err(CoreError::invalid(
                "failed compensation requires an explicit resume action",
            ));
        }
        let operation = self
            .storage
            .get_current_discovery_operation(session_id)?
            .ok_or_else(|| CoreError::invalid("compensation has no active operation"))?;
        if operation.kind != DiscoveryOperationKind::Compensation {
            return Err(CoreError::invalid(
                "active discovery operation is not compensation",
            ));
        }
        if operation.status == DiscoveryOperationStatus::Prepared
            && !self
                .storage
                .mark_discovery_operation_started(&operation.id, Utc::now())?
        {
            return Err(CoreError::invalid(
                "compensation operation changed concurrently",
            ));
        } else if !matches!(
            operation.status,
            DiscoveryOperationStatus::Prepared | DiscoveryOperationStatus::Started
        ) {
            return Err(CoreError::invalid("compensation operation is not active"));
        }
        let attempt_id = snapshot
            .session
            .commit_attempt_id
            .as_ref()
            .ok_or_else(|| CoreError::internal("compensation lost its commit attempt"))?
            .clone();
        loop {
            let steps = self
                .storage
                .list_discovery_compensation_steps(&attempt_id)?;
            let Some(step) = steps.iter().find(|step| {
                step.status != lorepia_storage::DiscoveryCompensationStatus::Completed
            }) else {
                let current = self.get(session_id)?;
                let mut draft = hydrate_working_draft(&current)?;
                let operation_id = current
                    .active_operation_id
                    .as_ref()
                    .ok_or_else(|| CoreError::invalid("compensation operation disappeared"))?;
                self.persist_operation_completion(
                    &current,
                    operation_id,
                    &mut draft,
                    ProviderDiscoveryAction::CompensationSucceeded,
                    DurableOperationOutcome::Succeeded,
                    Vec::new(),
                    Vec::new(),
                    DiscoveryJsonUpdate::Preserve,
                )?;
                return self.get(session_id);
            };
            match step.status {
                lorepia_storage::DiscoveryCompensationStatus::Failed => {
                    return Err(CoreError::invalid(
                        "failed compensation step requires an explicit resume",
                    ));
                }
                lorepia_storage::DiscoveryCompensationStatus::OutcomeUnknown => {
                    return Err(CoreError::invalid(
                        "unknown compensation outcome requires explicit reconciliation",
                    ));
                }
                lorepia_storage::DiscoveryCompensationStatus::Pending => {
                    if step.kind == DiscoveryCompensationKind::RemoveCredentialSlot {
                        return self.get(session_id);
                    }
                    self.storage.update_discovery_compensation_status(
                        &step.id,
                        lorepia_storage::DiscoveryCompensationStatus::Pending,
                        lorepia_storage::DiscoveryCompensationStatus::InProgress,
                        None,
                        Utc::now(),
                    )?;
                }
                lorepia_storage::DiscoveryCompensationStatus::InProgress => {
                    if step.kind == DiscoveryCompensationKind::RemoveCredentialSlot {
                        return self.get(session_id);
                    }
                }
                lorepia_storage::DiscoveryCompensationStatus::Completed => continue,
            }
            let result = match step.kind {
                DiscoveryCompensationKind::RemoveConnectionGraph => self
                    .storage
                    .compensate_discovered_provider_graph(&attempt_id, Utc::now()),
                DiscoveryCompensationKind::RestorePreviousSelection => self
                    .storage
                    .restore_discovery_previous_selection(&attempt_id, Utc::now()),
                DiscoveryCompensationKind::RemoveCredentialSlot => return self.get(session_id),
            };
            if result.is_err() {
                return self.persist_compensation_failure(
                    session_id,
                    &step.id,
                    DiscoveryFailure {
                        code: "compensation_database_step_failed".to_owned(),
                        message_key: "provider.discovery.compensation_database_step_failed"
                            .to_owned(),
                        recoverable: true,
                    },
                );
            }
        }
    }

    pub fn start_credential_compensation(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<lorepia_storage::DiscoveryCompensationRecord> {
        self.continue_compensation(session_id)?;
        let step = self.require_credential_compensation_step(session_id, step_id)?;
        let expected = match step.status {
            lorepia_storage::DiscoveryCompensationStatus::Pending
            | lorepia_storage::DiscoveryCompensationStatus::Failed => step.status,
            _ => {
                return Err(CoreError::invalid(
                    "credential compensation step cannot be started",
                ));
            }
        };
        self.storage.update_discovery_compensation_status(
            step_id,
            expected,
            lorepia_storage::DiscoveryCompensationStatus::InProgress,
            None,
            Utc::now(),
        )?;
        self.require_credential_compensation_step(session_id, step_id)
    }

    pub fn complete_credential_compensation(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.require_credential_compensation_step(session_id, step_id)?;
        self.storage.update_discovery_compensation_status(
            step_id,
            lorepia_storage::DiscoveryCompensationStatus::InProgress,
            lorepia_storage::DiscoveryCompensationStatus::Completed,
            None,
            Utc::now(),
        )?;
        self.continue_compensation(session_id)
    }

    pub fn fail_credential_compensation(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
        failure: DiscoveryFailure,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        failure.validate().map_err(|error| {
            CoreError::invalid(format!("invalid compensation failure: {error}"))
        })?;
        self.require_credential_compensation_step(session_id, step_id)?;
        self.persist_compensation_failure(session_id, step_id, failure)
    }

    fn persist_compensation_failure(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
        failure: DiscoveryFailure,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let operation_id = snapshot
            .active_operation_id
            .as_ref()
            .ok_or_else(|| CoreError::invalid("compensation operation disappeared"))?
            .clone();
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::CompensationFailed { failure },
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        self.storage
            .fail_discovery_compensation_and_persist_transition(
                step_id,
                &DiscoveryTransitionWrite {
                    transition,
                    draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
                    review: DiscoveryJsonUpdate::Preserve,
                    new_evidence: Vec::new(),
                    new_candidates: Vec::new(),
                    approval: None,
                    new_operation_id: None,
                    completed_operation: Some(DiscoveryCompletedOperationWrite {
                        id: operation_id,
                        outcome: DurableOperationOutcome::Failed,
                    }),
                    prepared_commit: None,
                    provider_graph: None,
                    occurred_at: Utc::now(),
                },
            )?;
        self.get(session_id)
    }

    pub fn mark_credential_compensation_unknown(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.require_credential_compensation_step(session_id, step_id)?;
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let operation_id = snapshot
            .active_operation_id
            .as_ref()
            .ok_or_else(|| CoreError::invalid("compensation operation disappeared"))?
            .clone();
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        self.storage
            .mark_discovery_compensation_unknown_and_persist_transition(
                step_id,
                &DiscoveryTransitionWrite {
                    transition,
                    draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
                    review: DiscoveryJsonUpdate::Preserve,
                    new_evidence: Vec::new(),
                    new_candidates: Vec::new(),
                    approval: None,
                    new_operation_id: None,
                    completed_operation: Some(DiscoveryCompletedOperationWrite {
                        id: operation_id,
                        outcome: DurableOperationOutcome::OutcomeUnknown,
                    }),
                    prepared_commit: None,
                    provider_graph: None,
                    occurred_at: Utc::now(),
                },
            )?;
        self.get(session_id)
    }

    pub fn resume_compensation(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::ResumeCompensation,
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        self.storage
            .persist_discovery_transition(&DiscoveryTransitionWrite {
                new_operation_id: Some(DiscoveryOperationId::new()),
                transition,
                draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
                review: DiscoveryJsonUpdate::Preserve,
                new_evidence: Vec::new(),
                new_candidates: Vec::new(),
                approval: None,
                completed_operation: None,
                prepared_commit: None,
                provider_graph: None,
                occurred_at: Utc::now(),
            })?;
        self.continue_compensation(session_id)
    }

    fn require_credential_compensation_step(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<lorepia_storage::DiscoveryCompensationRecord> {
        let snapshot = self.get(session_id)?;
        let attempt_id = snapshot
            .session
            .commit_attempt_id
            .as_ref()
            .ok_or_else(|| CoreError::invalid("discovery has no commit attempt"))?;
        self.storage
            .list_discovery_compensation_steps(attempt_id)?
            .into_iter()
            .find(|step| {
                step.id == step_id && step.kind == DiscoveryCompensationKind::RemoveCredentialSlot
            })
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "credential compensation step was not found",
                    false,
                )
            })
    }

    pub fn credential_install_context(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        self.credential_install_context_inner(session_id, false)
    }

    /// Returns the exact authority for a temporary discovery credential.
    ///
    /// The authority is available only before the provider graph is committed.
    /// Before origin approval it projects the exact scope that the approval
    /// action will apply. Afterwards it requires the immutable approved record
    /// and the current connection draft to still describe that same scope.
    pub fn credential_lease_context(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialLeaseContext> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.cancellation_pending
            || !discovery_state_accepts_credential_lease(&snapshot.session)
        {
            return Err(CoreError::invalid(
                "provider discovery is not accepting a pre-commit credential lease",
            ));
        }
        let draft = hydrate_working_draft(&snapshot)?;
        let template = draft
            .template
            .as_ref()
            .ok_or_else(|| CoreError::internal("credential lease has no template draft"))?;
        if template.default_manifest.auth == AuthBinding::None {
            return Err(CoreError::invalid(
                "provider discovery does not require a credential lease",
            ));
        }
        let current_connection = draft
            .connection
            .as_ref()
            .ok_or_else(|| CoreError::internal("credential lease has no connection draft"))?;
        require_discovery_credential_reference(&snapshot, current_connection)?;

        let (approval_id, grant_sha256, connection_binding_sha256) =
            if snapshot.session.state == DiscoveryState::AwaitingCredentialOriginApproval {
                if draft.credential_approval_id.is_some()
                    || current_connection.credential_scope.is_some()
                {
                    return Err(CoreError::invalid(
                        "credential lease was scoped before origin approval",
                    ));
                }
                let proposal = credential_origin_proposal(&snapshot, &draft)?;
                (
                    proposal.id,
                    proposal.grant_sha256,
                    canonical_discovery_credential_binding_sha256(&snapshot, &draft)?,
                )
            } else {
                let approval_id = draft.credential_approval_id.as_ref().ok_or_else(|| {
                    CoreError::invalid("credential lease has no durable origin approval")
                })?;
                let approval = self
                    .storage
                    .list_discovery_approvals(&snapshot.session.id, MAX_DISCOVERY_ROWS)?
                    .into_iter()
                    .find(|approval| &approval.id == approval_id)
                    .ok_or_else(|| {
                        CoreError::invalid("credential lease origin approval record is missing")
                    })?;
                validate_credential_origin_approval(&snapshot, &draft, &approval)?;
                let current_binding_sha256 = validated_discovery_credential_binding_sha256(
                    &snapshot,
                    &draft,
                    current_connection,
                )?;
                (
                    approval.id,
                    canonical_serde_sha256(&approval.grant, "credential-origin approval grant")?,
                    current_binding_sha256,
                )
            };

        Ok(ProviderDiscoveryCredentialLeaseContext {
            session_id: snapshot.session.id,
            connection_id: current_connection.id.clone(),
            credential_api_origin: current_connection.api_origin.clone(),
            credential_origin_approval_id: approval_id,
            credential_origin_grant_sha256: grant_sha256,
            connection_binding_sha256,
        })
    }

    /// Returns the exact install binding during startup cancellation recovery.
    ///
    /// This does not authorize a new vault write. It exists only so the native
    /// host can compare a physically-bound WAL operation with the current vault
    /// status before Core performs its conservative generic recovery. A sealed
    /// pre-schema-37 Started lineage is returned without physical authority so
    /// the host must defer it to outcome-unknown recovery.
    pub fn credential_install_recovery_context(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        self.credential_install_context_inner(session_id, true)
    }

    fn validate_credential_install_context_authority(
        &self,
        recovery_context: bool,
        session_id: &DiscoverySessionId,
        attempt_id: &DiscoveryCommitAttemptId,
        plan_sha256: &str,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<()> {
        if recovery_context {
            self.storage
                .validate_discovery_credential_install_recovery_authority(
                    session_id,
                    attempt_id,
                    plan_sha256,
                    operation_id,
                )
        } else {
            self.storage
                .validate_discovery_credential_install_operation_authority(
                    session_id,
                    attempt_id,
                    plan_sha256,
                    operation_id,
                )
        }
    }

    fn credential_install_context_inner(
        &self,
        session_id: &DiscoverySessionId,
        recovery_context: bool,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::Committing
            || (!recovery_context && snapshot.session.cancellation_pending)
        {
            return Err(CoreError::invalid(
                "provider discovery is not accepting a credential installation",
            ));
        }
        let attempt_id = snapshot
            .session
            .commit_attempt_id
            .as_ref()
            .ok_or_else(|| CoreError::internal("credential commit has no attempt"))?;
        let plan_sha256 = snapshot
            .session
            .commit_plan_sha256
            .as_ref()
            .ok_or_else(|| CoreError::internal("credential commit has no plan hash"))?;
        let attempt = self.storage.get_discovery_commit_attempt(attempt_id)?;
        let operation = self
            .storage
            .get_current_discovery_operation(session_id)?
            .ok_or_else(|| CoreError::internal("credential commit has no active operation"))?;
        let credential_reference =
            attempt.plan.credential_ref.as_ref().ok_or_else(|| {
                CoreError::invalid("discovery commit does not require a credential")
            })?;
        if snapshot.active_operation_id.as_ref() != Some(&operation.id)
            || operation.kind != DiscoveryOperationKind::AtomicCommit
            || !matches!(
                operation.status,
                DiscoveryOperationStatus::Prepared | DiscoveryOperationStatus::Started
            )
            || attempt.session_id != snapshot.session.id
            || attempt.plan.session_id != snapshot.session.id
            || !(operation.expected_revision == snapshot.session.revision
                || (recovery_context && snapshot.session.cancellation_pending))
            || attempt.plan_sha256 != *plan_sha256
            || attempt.plan.attempt_id != *attempt_id
            || attempt.plan.connection_id != snapshot.session.input.connection_id
            || credential_reference.as_str() != attempt.plan.connection_id.as_str()
        {
            return Err(CoreError::invalid(
                "credential installation is detached from its approved commit attempt",
            ));
        }
        self.validate_credential_install_context_authority(
            recovery_context,
            &snapshot.session.id,
            &attempt.id,
            &attempt.plan_sha256,
            &operation.id,
        )?;
        let draft = hydrate_working_draft(&snapshot)?;
        let working_connection = draft
            .connection
            .as_ref()
            .ok_or_else(|| CoreError::internal("credential commit has no connection draft"))?;
        if working_connection.id != attempt.plan.connection_id {
            return Err(CoreError::invalid(
                "credential installation connection differs from its approved commit",
            ));
        }
        let connection_binding_sha256 =
            validated_discovery_credential_binding_sha256(&snapshot, &draft, working_connection)?;
        let native_execution = self
            .storage
            .get_discovery_native_credential_execution(&operation.id)?;
        if native_execution.as_ref().is_some_and(|execution| {
            execution.operation_id != operation.id
                || execution.session_id != snapshot.session.id
                || execution.commit_attempt_id != attempt.id
                || execution.commit_plan_sha256 != attempt.plan_sha256
                || execution.connection_id != attempt.plan.connection_id
                || execution.connection_binding_sha256 != connection_binding_sha256
        }) {
            return Err(CoreError::invalid(
                "native credential execution differs from its approved commit",
            ));
        }
        let (native_execution_reservation_id, native_execution_id) =
            native_credential_execution_context_ids(
                operation.status,
                operation.started_at.as_ref(),
                native_execution,
                recovery_context,
            )?;
        Ok(ProviderDiscoveryCredentialInstallContext {
            session_id: snapshot.session.id,
            session_revision: snapshot.session.revision,
            operation_id: operation.id,
            operation_status: operation.status,
            native_execution_reservation_id,
            native_execution_id,
            commit_attempt_id: attempt.id,
            commit_plan_sha256: attempt.plan_sha256,
            commit_phase: attempt.phase,
            connection_id: attempt.plan.connection_id,
            connection_binding_sha256,
        })
    }

    pub fn credential_compensation_authority(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialAuthority> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::Compensating {
            return Err(CoreError::invalid(
                "provider discovery is not compensating a credential installation",
            ));
        }
        let attempt_id = snapshot
            .session
            .commit_attempt_id
            .as_ref()
            .ok_or_else(|| CoreError::internal("credential compensation has no attempt"))?;
        let attempt = self.storage.get_discovery_commit_attempt(attempt_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let connection = draft.connection.as_ref().ok_or_else(|| {
            CoreError::internal("credential compensation has no connection draft")
        })?;
        if attempt.session_id != snapshot.session.id
            || attempt.plan.attempt_id != *attempt_id
            || attempt.plan.connection_id != connection.id
            || attempt
                .plan
                .credential_ref
                .as_ref()
                .map(CredentialRef::as_str)
                != Some(connection.id.as_str())
        {
            return Err(CoreError::invalid(
                "credential compensation differs from its immutable commit attempt",
            ));
        }
        let operation_id = self
            .storage
            .get_discovery_credential_compensation_operation_id(
                &snapshot.session.id,
                &attempt.id,
                &attempt.plan_sha256,
            )?;
        let connection_binding_sha256 =
            validated_discovery_credential_binding_sha256(&snapshot, &draft, connection)?;
        let (credential_origin_approval_id, credential_origin_grant_sha256) =
            approved_discovery_credential_origin_authority(self.storage, &snapshot, &draft)?;
        if attempt.plan.credential_approval_id.as_ref() != Some(&credential_origin_approval_id) {
            return Err(CoreError::invalid(
                "credential compensation origin approval differs from its immutable commit",
            ));
        }
        let native_execution = self
            .storage
            .get_discovery_native_credential_execution(&operation_id)?
            .ok_or_else(|| {
                CoreError::invalid(
                    "credential compensation has no producing native execution authority",
                )
            })?;
        if native_execution.operation_id != operation_id
            || native_execution.session_id != snapshot.session.id
            || native_execution.commit_attempt_id != attempt.id
            || native_execution.commit_plan_sha256 != attempt.plan_sha256
            || native_execution.connection_id != connection.id
            || native_execution.connection_binding_sha256 != connection_binding_sha256
            || native_execution.store_started_at.is_none()
        {
            return Err(CoreError::invalid(
                "credential compensation native execution differs from its immutable commit",
            ));
        }
        Ok(ProviderDiscoveryCredentialAuthority {
            operation_id,
            native_execution_id: native_execution.physical_authority_id,
            commit_attempt_id: attempt.id,
            connection_id: connection.id.clone(),
            credential_api_origin: connection.api_origin.clone(),
            credential_origin_approval_id,
            credential_origin_grant_sha256,
            connection_binding_sha256,
        })
    }

    /// Reserves a fresh physical slot incarnation while the semantic operation
    /// remains Prepared. Native fallible preconditions run against this exact
    /// reservation before any durable store-attempt intent is recorded.
    pub fn reserve_credential_install(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        let context = self.credential_install_context(session_id)?;
        if context.session_revision != expected_revision
            || &context.operation_id != expected_operation_id
            || &context.commit_attempt_id != expected_attempt_id
            || context.commit_plan_sha256 != expected_plan_sha256
            || context.commit_phase != DiscoveryCommitPhase::Prepared
            || context.operation_status != DiscoveryOperationStatus::Prepared
            || context.native_execution_id.is_some()
        {
            return Err(CoreError::invalid(
                "credential installation context changed before native reservation",
            ));
        }
        if context.native_execution_reservation_id.is_some() {
            return Err(CoreError::invalid(
                "prepared credential reservation already exists and requires recovery",
            ));
        }
        let reservation = DiscoveryNativeCredentialExecutionReservation {
            operation_id: context.operation_id.clone(),
            session_id: context.session_id.clone(),
            commit_attempt_id: context.commit_attempt_id.clone(),
            commit_plan_sha256: context.commit_plan_sha256.clone(),
            connection_id: context.connection_id.clone(),
            connection_binding_sha256: context.connection_binding_sha256.clone(),
            reserved_at: Utc::now(),
        };
        let execution = self
            .storage
            .reserve_discovery_credential_install_execution(&reservation)?;
        let reserved = self.credential_install_context(session_id)?;
        if reserved.operation_id != context.operation_id
            || reserved.operation_status != DiscoveryOperationStatus::Prepared
            || reserved.commit_phase != DiscoveryCommitPhase::Prepared
            || reserved.native_execution_id.is_some()
            || reserved.native_execution_reservation_id.as_deref()
                != Some(execution.physical_authority_id.as_str())
        {
            return Err(CoreError::internal(
                "credential installation reservation was not durably bound",
            ));
        }
        Ok(reserved)
    }

    /// Durably records that the exact reserved physical slot is the next
    /// external action, and atomically moves the semantic operation to Started.
    pub fn start_credential_install(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
        expected_native_execution_reservation_id: &str,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        let context = self.credential_install_context(session_id)?;
        if context.session_revision != expected_revision
            || &context.operation_id != expected_operation_id
            || &context.commit_attempt_id != expected_attempt_id
            || context.commit_plan_sha256 != expected_plan_sha256
            || context.commit_phase != DiscoveryCommitPhase::Prepared
            || context.operation_status != DiscoveryOperationStatus::Prepared
            || context.native_execution_id.is_some()
            || context.native_execution_reservation_id.as_deref()
                != Some(expected_native_execution_reservation_id)
        {
            return Err(CoreError::invalid(
                "credential installation reservation changed before native store",
            ));
        }
        let execution_start = DiscoveryNativeCredentialStoreAttemptStart {
            operation_id: context.operation_id.clone(),
            physical_authority_id: expected_native_execution_reservation_id.to_owned(),
            started_at: Utc::now(),
        };
        let execution = self
            .storage
            .start_reserved_discovery_credential_install_execution(&execution_start)?;
        let started = self.credential_install_context(session_id)?;
        if started.operation_id != context.operation_id
            || started.operation_status != DiscoveryOperationStatus::Started
            || started.commit_phase != DiscoveryCommitPhase::Prepared
            || started.native_execution_reservation_id.as_deref()
                != Some(execution.physical_authority_id.as_str())
            || started.native_execution_id.as_deref()
                != Some(execution.physical_authority_id.as_str())
        {
            return Err(CoreError::internal(
                "credential installation start was not durably bound",
            ));
        }
        Ok(started)
    }

    /// Records a platform-attested missing vault slot after an installation
    /// attempt, without guessing or retrying an external effect.
    pub fn attest_credential_install_no_effect(
        &self,
        session_id: &DiscoverySessionId,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
        expected_native_execution_id: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        if self.recovery_owner != DiscoveryRecoveryOwner::NativePlatform {
            return Err(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "native credential no-effect attestation requires the native recovery owner",
                false,
            ));
        }
        let context = self.credential_install_context_inner(session_id, true)?;
        if &context.operation_id != expected_operation_id
            || &context.commit_attempt_id != expected_attempt_id
            || context.commit_plan_sha256 != expected_plan_sha256
            || context.commit_phase != DiscoveryCommitPhase::Prepared
            || context.operation_status != DiscoveryOperationStatus::Started
            || context.native_execution_id.as_deref() != Some(expected_native_execution_id)
        {
            return Err(CoreError::invalid(
                "credential no-effect attestation does not match the active commit",
            ));
        }
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        self.persist_native_no_effect_completion(
            &snapshot,
            &context.operation_id,
            &mut draft,
            ProviderDiscoveryAction::Interrupt {
                operation: DiscoveryOperationKind::AtomicCommit,
                outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
            },
            DurableOperationOutcome::AttestedNoExternalEffect,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
            &context,
        )?;
        self.get(session_id)
    }

    /// Durably records that the exact native credential store attempt reported
    /// an explicit durability failure after it may have mutated its slot.
    ///
    /// Immediate vault visibility is deliberately not accepted here: a native
    /// platform can expose the new bytes while failing the directory/fsync
    /// boundary needed to survive a crash. The complete immutable execution
    /// authority is compared before the active atomic-commit operation is
    /// closed as outcome-unknown in the same discovery transaction.
    #[allow(clippy::too_many_arguments)]
    pub fn mark_credential_install_durability_unknown(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
        expected_native_execution_id: &str,
        expected_connection_id: &ProviderConnectionId,
        expected_connection_binding_sha256: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        if self.recovery_owner != DiscoveryRecoveryOwner::NativePlatform {
            return Err(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "native credential durability attestation requires the native recovery owner",
                false,
            ));
        }
        let context = self.credential_install_context_inner(session_id, true)?;
        if context.session_revision != expected_revision
            || &context.operation_id != expected_operation_id
            || &context.commit_attempt_id != expected_attempt_id
            || context.commit_plan_sha256 != expected_plan_sha256
            || context.commit_phase != DiscoveryCommitPhase::Prepared
            || context.operation_status != DiscoveryOperationStatus::Started
            || context.native_execution_reservation_id.as_deref()
                != Some(expected_native_execution_id)
            || context.native_execution_id.as_deref() != Some(expected_native_execution_id)
            || &context.connection_id != expected_connection_id
            || context.connection_binding_sha256 != expected_connection_binding_sha256
        {
            return Err(CoreError::invalid(
                "credential durability failure does not match the active native commit",
            ));
        }
        let snapshot = self.get(session_id)?;
        if snapshot.session.revision != expected_revision
            || snapshot.active_operation_id.as_ref() != Some(expected_operation_id)
        {
            return Err(CoreError::invalid(
                "credential durability failure changed before settlement",
            ));
        }
        let mut draft = hydrate_working_draft(&snapshot)?;
        self.persist_operation_completion(
            &snapshot,
            expected_operation_id,
            &mut draft,
            ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
            DurableOperationOutcome::OutcomeUnknown,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )?;
        self.get(session_id)
    }

    /// Executes the already-approved atomic graph publication. For a graph
    /// carrying an opaque native credential reference, the caller must confirm
    /// that the reference exists in the native vault; the raw credential is
    /// never accepted here.
    pub fn commit(
        &self,
        session_id: &DiscoverySessionId,
        credential_confirmation: Option<&ProviderDiscoveryCredentialCommitConfirmation>,
    ) -> CoreResult<ProviderConnection> {
        let snapshot = self.get(session_id)?;
        require_active_discovery_commit_authority(&snapshot)?;
        let operation_id = snapshot
            .active_operation_id
            .clone()
            .ok_or_else(|| CoreError::internal("committing discovery has no active operation"))?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let attempt_id =
            snapshot.session.commit_attempt_id.as_ref().ok_or_else(|| {
                CoreError::internal("committing discovery lost its commit attempt")
            })?;
        let attempt = self.storage.get_discovery_commit_attempt(attempt_id)?;
        revalidate_prepared_discovery_catalog_authority(self.storage, &draft, attempt.phase)?;
        let graph = graph_from_plan(&draft, attempt.plan, attempt.plan_sha256)?;
        let credential_bound = graph.connection.credential_ref.is_some();
        if !credential_bound && credential_confirmation.is_some() {
            return Err(CoreError::invalid(
                "credentialless discovery cannot accept a native credential confirmation",
            ));
        }
        if credential_bound {
            self.require_commit_operation_started(&snapshot, &operation_id)?;
        } else {
            self.ensure_commit_operation_started(&snapshot, &operation_id)?;
        }
        if snapshot.session.cancellation_pending {
            self.settle_started_commit_cancellation(&snapshot, &operation_id)?;
            return Err(cancelled_commit_error());
        }
        if credential_bound {
            self.require_exact_credential_commit_confirmation(session_id, credential_confirmation)?;
        }

        let current = self.get(session_id)?;
        if current.session.state != DiscoveryState::Committing {
            return Err(CoreError::invalid(
                "provider discovery changed while the atomic commit was starting",
            ));
        }
        if current.session.cancellation_pending {
            self.settle_started_commit_cancellation(&current, &operation_id)?;
            return Err(cancelled_commit_error());
        }
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            current.session.revision,
            ProviderDiscoveryAction::CommitSucceeded {
                connection_id: graph.connection.id.clone(),
            },
        )?;
        let transition = current.session.apply(&envelope).map_err(transition_error)?;
        let write = DiscoveryTransitionWrite {
            transition,
            draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
            review: DiscoveryJsonUpdate::Preserve,
            new_evidence: Vec::new(),
            new_candidates: Vec::new(),
            approval: None,
            new_operation_id: None,
            completed_operation: Some(DiscoveryCompletedOperationWrite {
                id: operation_id.clone(),
                outcome: DurableOperationOutcome::Succeeded,
            }),
            prepared_commit: None,
            provider_graph: Some(graph.clone()),
            occurred_at: Utc::now(),
        };
        let persisted = if graph.connection.credential_ref.is_none() {
            self.storage.persist_discovery_transition(&write)
        } else {
            self.storage
                .persist_credential_confirmed_discovery_commit(&write)
        };
        if let Err(error) = persisted {
            let latest = self.get(session_id)?;
            if latest.session.state == DiscoveryState::Committing
                && latest.session.cancellation_pending
            {
                self.settle_started_commit_cancellation(&latest, &operation_id)?;
                return Err(cancelled_commit_error());
            }
            return Err(error);
        }
        let ready = self.get(session_id)?;
        if !matches!(
            ready.session.state,
            DiscoveryState::Ready | DiscoveryState::Compensating
        ) {
            return Err(CoreError::internal(
                "provider discovery commit reached neither ready nor compensation",
            ));
        }
        draft
            .connection
            .take()
            .ok_or_else(|| CoreError::internal("committed discovery lost its provider connection"))
    }

    fn require_exact_credential_commit_confirmation(
        &self,
        session_id: &DiscoverySessionId,
        confirmation: Option<&ProviderDiscoveryCredentialCommitConfirmation>,
    ) -> CoreResult<()> {
        let confirmation = confirmation.ok_or_else(|| {
            CoreError::invalid("native credential reference confirmation is required")
        })?;
        let current_context = self.credential_install_context(session_id)?;
        let expected_confirmation =
            ProviderDiscoveryCredentialCommitConfirmation::try_from(&current_context)?;
        if confirmation != &expected_confirmation
            || current_context.operation_status != DiscoveryOperationStatus::Started
            || current_context.commit_phase != DiscoveryCommitPhase::Prepared
        {
            return Err(CoreError::invalid(
                "native credential confirmation does not match the active commit operation",
            ));
        }
        Ok(())
    }

    fn ensure_commit_operation_started(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<()> {
        if self
            .storage
            .mark_discovery_operation_started(operation_id, Utc::now())?
        {
            return Ok(());
        }
        let operation = self
            .storage
            .get_current_discovery_operation(&snapshot.session.id)?
            .ok_or_else(|| CoreError::invalid("atomic discovery commit operation disappeared"))?;
        if operation.id == *operation_id && operation.status == DiscoveryOperationStatus::Started {
            Ok(())
        } else {
            Err(CoreError::invalid(
                "atomic discovery commit already completed or changed",
            ))
        }
    }

    fn require_commit_operation_started(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<()> {
        let operation = self
            .storage
            .get_current_discovery_operation(&snapshot.session.id)?
            .ok_or_else(|| CoreError::invalid("atomic discovery commit operation disappeared"))?;
        if snapshot.active_operation_id.as_ref() == Some(operation_id)
            && operation.id == *operation_id
            && operation.kind == DiscoveryOperationKind::AtomicCommit
            && operation.status == DiscoveryOperationStatus::Started
        {
            Ok(())
        } else {
            Err(CoreError::invalid(
                "credential-bound discovery commit was not explicitly started",
            ))
        }
    }

    fn settle_started_commit_cancellation(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<()> {
        if snapshot.session.state != DiscoveryState::Committing
            || !snapshot.session.cancellation_pending
        {
            return Err(CoreError::invalid(
                "atomic discovery commit has no pending cancellation",
            ));
        }
        let mut draft = hydrate_working_draft(snapshot)?;
        self.persist_operation_completion(
            snapshot,
            operation_id,
            &mut draft,
            ProviderDiscoveryAction::CompensationRequired,
            DurableOperationOutcome::Failed,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )?;
        self.continue_compensation(&snapshot.session.id)?;
        Ok(())
    }

    pub fn approval_proposal(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<ProviderDiscoveryApprovalProposal>> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        proposal_for_state(&snapshot, &draft).transpose()
    }

    pub fn review_proposal(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<ProviderDiscoveryReviewProposal>> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::AwaitingReview {
            return Ok(None);
        }
        let draft = hydrate_working_draft(&snapshot)?;
        let review = snapshot
            .review
            .clone()
            .ok_or_else(|| CoreError::internal("review state has no persisted diff"))?;
        let plan = commit_plan_for(
            self.storage,
            &snapshot,
            &draft,
            deterministic_commit_attempt_id(&snapshot.session.id, snapshot.session.revision),
            &review,
        )?;
        let commit_plan_sha256 = canonical_serde_sha256(&plan, "discovery commit plan")?;
        let approval = approval_proposal_for(
            &snapshot.session.id,
            snapshot.session.revision,
            DiscoveryApprovalGrant::Review {
                review_sha256: review.sha256.clone(),
                graph_sha256: review.graph_sha256.clone(),
            },
        )?;
        let request_preview = match (
            draft.template.as_ref(),
            draft.connection.as_ref(),
            draft.routes.first(),
        ) {
            (Some(template), Some(connection), Some(route)) => Some(
                AdapterRegistry::new()
                    .preview_provider_request(template, connection, route, None)?,
            ),
            _ => None,
        };
        Ok(Some(ProviderDiscoveryReviewProposal {
            review,
            approval,
            commit_attempt_id: plan.attempt_id,
            commit_plan_sha256,
            request_preview,
        }))
    }

    pub fn begin_assistant_turn(
        &self,
        session_id: &DiscoverySessionId,
        estimate: AssistantCallEstimate,
    ) -> CoreResult<AssistantPromptPackage> {
        let snapshot = self.get(session_id)?;
        let operation = self
            .storage
            .get_current_discovery_operation(session_id)?
            .ok_or_else(|| CoreError::invalid("assistant discovery has no active operation"))?;
        if operation.kind != DiscoveryOperationKind::BuildAssistantManifestDraft {
            return Err(CoreError::invalid(
                "provider discovery is not running the setup assistant",
            ));
        }
        if operation.status == lorepia_storage::DiscoveryOperationStatus::Prepared
            && !self
                .storage
                .mark_discovery_operation_started(&operation.id, Utc::now())?
        {
            return Err(CoreError::invalid(
                "setup assistant operation changed concurrently",
            ));
        }
        if !matches!(
            operation.status,
            lorepia_storage::DiscoveryOperationStatus::Prepared
                | lorepia_storage::DiscoveryOperationStatus::Started
        ) {
            return Err(CoreError::invalid(
                "setup assistant operation is not active",
            ));
        }
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        let prompt = engine.begin_turn(estimate).map_err(assistant_error)?;
        synchronize_assistant_snapshot(&mut draft, &engine);
        self.persist_assistant_checkpoint(
            &snapshot,
            &draft,
            DiscoveryAssistantCheckpoint::AwaitingAssistant,
        )?;
        Ok(prompt)
    }

    fn run_assistant_with_provider(
        &self,
        session_id: &DiscoverySessionId,
        route: &ModelRoute,
        provider: Arc<dyn Provider>,
        estimate: AssistantCallEstimate,
        credential: Option<&str>,
    ) -> CoreResult<AssistantHostAction> {
        for _ in 0..MAX_ASSISTANT_HOST_STEPS {
            let prompt = self.begin_assistant_turn(session_id, estimate)?;
            let output = self.runtime.block_on(run_setup_assistant_provider_call(
                Arc::clone(&provider),
                route,
                &prompt,
                estimate,
                credential,
            ));
            let action = match output {
                Ok(turn) => self.submit_assistant_turn(session_id, turn)?,
                Err(error) => {
                    let failure_kind = assistant_failure_kind(&error);
                    let retryable = error.recoverable
                        || matches!(
                            error.code,
                            CoreErrorCode::ProviderRateLimited
                                | CoreErrorCode::ProviderUnavailable
                                | CoreErrorCode::NetworkUnavailable
                        );
                    self.record_assistant_failure(session_id, failure_kind, retryable)?;
                    return Err(error);
                }
            };
            match action {
                AssistantHostAction::ExecuteTool {
                    session_id: action_session_id,
                    call_id,
                    call,
                } => {
                    if action_session_id != *session_id {
                        self.interrupt_assistant(
                            session_id,
                            DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                        )?;
                        return Err(CoreError::internal(
                            "setup assistant tool action escaped its discovery session",
                        ));
                    }
                    let result = match self.execute_assistant_tool(session_id, &call) {
                        Ok(result) => result,
                        Err(error) => {
                            self.interrupt_assistant(
                                session_id,
                                DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                            )?;
                            return Err(error);
                        }
                    };
                    if let Err(error) =
                        self.submit_assistant_tool_result(session_id, call_id, result)
                    {
                        self.interrupt_assistant(
                            session_id,
                            DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                        )?;
                        return Err(error);
                    }
                }
                boundary => return Ok(boundary),
            }
        }
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let operation_id = snapshot
            .active_operation_id
            .as_ref()
            .ok_or_else(|| CoreError::invalid("setup assistant operation disappeared"))?
            .clone();
        self.persist_operation_completion(
            &snapshot,
            &operation_id,
            &mut draft,
            ProviderDiscoveryAction::Fail {
                failure: DiscoveryFailure {
                    code: "assistant_host_loop_exhausted".to_owned(),
                    message_key: "provider.discovery.assistant_host_loop_exhausted".to_owned(),
                    recoverable: false,
                },
            },
            DurableOperationOutcome::Failed,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )?;
        Err(CoreError::invalid(
            "setup assistant exceeded its bounded host-action loop",
        ))
    }

    #[allow(clippy::too_many_lines)]
    fn execute_assistant_tool(
        &self,
        session_id: &DiscoverySessionId,
        call: &AssistantToolCall,
    ) -> CoreResult<AssistantToolResult> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let allowed_evidence_ids = draft
            .evidence_ids
            .iter()
            .chain(&draft.extra_evidence_ids)
            .cloned()
            .collect::<BTreeSet<_>>();
        match call {
            AssistantToolCall::SearchOfficialDocs { query } => {
                let query = query.to_lowercase();
                let evidence_ids = self
                    .storage
                    .list_discovery_evidence(session_id, MAX_DISCOVERY_ROWS)?
                    .into_iter()
                    .filter(|record| allowed_evidence_ids.contains(&record.id))
                    .filter(|record| {
                        serde_json::to_string(&record.extracted_json)
                            .ok()
                            .is_some_and(|value| value.to_lowercase().contains(&query))
                    })
                    .take(128)
                    .map(|record| record.id)
                    .collect();
                Ok(AssistantToolResult::OfficialDocsSearch { evidence_ids })
            }
            AssistantToolCall::InspectEvidence { evidence_id } => {
                if !allowed_evidence_ids.contains(evidence_id) {
                    return Err(CoreError::invalid(
                        "setup assistant requested evidence outside this session",
                    ));
                }
                let record = self
                    .storage
                    .list_discovery_evidence(session_id, MAX_DISCOVERY_ROWS)?
                    .into_iter()
                    .find(|record| record.id == *evidence_id)
                    .ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::NotFound,
                            "setup assistant evidence was not found",
                            false,
                        )
                    })?;
                let claims = draft
                    .assistant_evidence_claims
                    .get(&record.id)
                    .cloned()
                    .unwrap_or_default();
                let supported_fields = redacted_assistant_evidence(record, claims)?
                    .claims()
                    .iter()
                    .map(|claim| claim.field().clone())
                    .collect();
                Ok(AssistantToolResult::EvidenceInspection {
                    evidence_id: evidence_id.clone(),
                    supported_fields,
                })
            }
            AssistantToolCall::FetchDiscoveryDocument { candidate_id } => {
                let entry = self
                    .storage
                    .read_discovery_candidates(session_id, MAX_DISCOVERY_ROWS)?
                    .into_iter()
                    .find(|entry| entry.candidate.id == *candidate_id)
                    .ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::NotFound,
                            "setup assistant document candidate was not found",
                            false,
                        )
                    })?;
                let evidence_ids = entry
                    .candidate
                    .evidence_ids
                    .into_iter()
                    .filter(|evidence_id| allowed_evidence_ids.contains(evidence_id))
                    .collect();
                Ok(AssistantToolResult::DiscoveryDocumentFetched {
                    candidate_id: candidate_id.clone(),
                    evidence_ids,
                })
            }
            AssistantToolCall::ListModels { connection_id } => {
                let connection = draft.connection.as_ref().ok_or_else(|| {
                    CoreError::invalid("setup assistant has no session-owned connection draft")
                })?;
                if connection.id != *connection_id {
                    return Err(CoreError::invalid(
                        "setup assistant requested models for another connection",
                    ));
                }
                Ok(AssistantToolResult::ModelsListed {
                    connection_id: connection_id.clone(),
                    model_route_ids: draft
                        .routes
                        .iter()
                        .map(|route| route.id.clone())
                        .take(128)
                        .collect(),
                })
            }
            AssistantToolCall::TestConnection { connection_id } => {
                let connection = draft.connection.as_ref().ok_or_else(|| {
                    CoreError::invalid("setup assistant has no session-owned connection draft")
                })?;
                if connection.id != *connection_id {
                    return Err(CoreError::invalid(
                        "setup assistant requested a test for another connection",
                    ));
                }
                let reachable = connection.status == ConnectionStatus::Connected;
                Ok(AssistantToolResult::ConnectionTested {
                    connection_id: connection_id.clone(),
                    reachable,
                    summary: if reachable {
                        "connected".to_owned()
                    } else {
                        "not_tested_before_origin_approval".to_owned()
                    },
                })
            }
            AssistantToolCall::ProbeCapability {
                model_route_id,
                capability,
            } => {
                if !draft.routes.iter().any(|route| route.id == *model_route_id) {
                    return Err(CoreError::invalid(
                        "setup assistant requested a capability for another model route",
                    ));
                }
                let observation = draft.observations.iter().rev().find(|observation| {
                    observation.model_route_id == *model_route_id
                        && observation.key == *capability
                        && observation.is_fresh_at(Utc::now())
                });
                let supported = observation.and_then(capability_observation_support);
                let evidence_ids = observation
                    .and_then(|observation| observation.evidence_ref.clone())
                    .filter(|evidence_id| allowed_evidence_ids.contains(evidence_id))
                    .into_iter()
                    .collect();
                Ok(AssistantToolResult::CapabilityProbed {
                    model_route_id: model_route_id.clone(),
                    capability: *capability,
                    supported,
                    evidence_ids,
                    summary: if observation.is_some() {
                        "existing_session_observation".to_owned()
                    } else {
                        "not_probed_before_capability_consent".to_owned()
                    },
                })
            }
            AssistantToolCall::ListManifestAdapterFamilies => {
                let mut families = draft
                    .deterministic
                    .as_ref()
                    .map(|output| {
                        output
                            .family_candidates
                            .iter()
                            .map(|candidate| candidate.api_family)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if families.is_empty() {
                    families = AdapterRegistry::built_in_templates()?
                        .into_iter()
                        .map(|template| template.api_family)
                        .collect();
                }
                families.sort_by_key(|family| api_family_slug(*family));
                families.dedup();
                Ok(AssistantToolResult::AdapterFamilies { families })
            }
            AssistantToolCall::ValidateManifestDraft { draft } => {
                let accepted = validate_manifest(&draft.manifest).is_ok();
                Ok(AssistantToolResult::ManifestValidation {
                    accepted,
                    violations: if accepted {
                        Vec::new()
                    } else {
                        vec!["manifest_rejected".to_owned()]
                    },
                })
            }
            AssistantToolCall::ShowUnresolvedQuestions => {
                Ok(AssistantToolResult::UnresolvedQuestions {
                    question_ids: self.current_assistant_unresolved_question_ids(
                        session_id,
                        snapshot.session.revision,
                    )?,
                })
            }
        }
    }

    fn current_assistant_unresolved_question_ids(
        &self,
        requested_session_id: &DiscoverySessionId,
        observed_revision: u64,
    ) -> CoreResult<Vec<String>> {
        let current = self.get(requested_session_id)?;
        let draft = hydrate_working_draft(&current)?;
        Self::validated_assistant_unresolved_question_ids(
            requested_session_id,
            observed_revision,
            &current,
            &draft,
        )
    }

    fn validated_assistant_unresolved_question_ids(
        requested_session_id: &DiscoverySessionId,
        observed_revision: u64,
        current: &DiscoverySessionSnapshot,
        draft: &DiscoveryWorkingDraft,
    ) -> CoreResult<Vec<String>> {
        const MAX_QUESTION_COUNT: usize = 128;
        const MAX_QUESTION_ID_BYTES: usize = 128;
        const MAX_QUESTION_TEXT_BYTES: usize = 2 * 1024;
        const MAX_TOOL_RESULT_BYTES: usize = 4 * 1024;

        let corrupted = || {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider setup assistant unresolved questions are inconsistent",
                false,
            )
        };
        if current.session.id != *requested_session_id
            || current.session.revision != observed_revision
            || current.session.state != DiscoveryState::BuildingAssistantManifestDraft
        {
            return Err(corrupted());
        }
        let assistant = draft.assistant.as_ref().ok_or_else(&corrupted)?;
        if assistant.session_id() != requested_session_id
            || assistant.state() != AssistantState::AwaitingToolResult
        {
            return Err(corrupted());
        }
        let engine = restored_assistant(draft).map_err(|_| corrupted())?;
        let questions = &draft.assistant_more_evidence_questions;
        if questions.is_empty() || questions.len() > MAX_QUESTION_COUNT {
            return Err(corrupted());
        }

        let mut question_ids = Vec::with_capacity(questions.len());
        let mut previous_id: Option<&str> = None;
        for question in questions {
            let id = question.id.as_str();
            if id.is_empty()
                || id.len() > MAX_QUESTION_ID_BYTES
                || !id.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
                })
                || previous_id.is_some_and(|previous| previous >= id)
                || question.question.trim().is_empty()
                || question.required_evidence.trim().is_empty()
                || question.question.len() > MAX_QUESTION_TEXT_BYTES
                || question.required_evidence.len() > MAX_QUESTION_TEXT_BYTES
                || question.question.bytes().any(|byte| byte == 0)
                || question.required_evidence.bytes().any(|byte| byte == 0)
            {
                return Err(corrupted());
            }
            previous_id = Some(id);
            question_ids.push(question.id.clone());
        }
        if engine.unresolved_question_ids() != question_ids {
            return Err(corrupted());
        }

        let result = AssistantToolResult::UnresolvedQuestions {
            question_ids: question_ids.clone(),
        };
        if serde_json::to_vec(&result).map_err(|_| corrupted())?.len() > MAX_TOOL_RESULT_BYTES {
            return Err(corrupted());
        }
        Ok(question_ids)
    }

    #[cfg(test)]
    fn submit_assistant_turn_json(
        &self,
        session_id: &DiscoverySessionId,
        output: &[u8],
    ) -> CoreResult<AssistantHostAction> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        let submission = engine.submit_turn_json(output);
        self.persist_assistant_submission(&snapshot, draft, engine, submission)
    }

    fn submit_assistant_turn(
        &self,
        session_id: &DiscoverySessionId,
        turn: lorepia_providers::setup_assistant::AssistantTurn,
    ) -> CoreResult<AssistantHostAction> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        let submission = engine.submit_turn(turn);
        self.persist_assistant_submission(&snapshot, draft, engine, submission)
    }

    fn persist_assistant_submission(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        mut draft: DiscoveryWorkingDraft,
        engine: SetupAssistantEngine,
        submission: Result<AssistantHostAction, AssistantError>,
    ) -> CoreResult<AssistantHostAction> {
        let state = engine.state();
        synchronize_assistant_snapshot(&mut draft, &engine);
        match submission {
            Ok(action) => {
                if let AssistantHostAction::RequestMoreEvidence { questions, .. } = &action {
                    let question_count = u32::try_from(questions.len()).map_err(|_| {
                        CoreError::invalid("setup assistant returned too many evidence questions")
                    })?;
                    if draft.assistant_more_evidence_questions != *questions {
                        return Err(corrupted_assistant_resume_boundary());
                    }
                    let operation_id = snapshot
                        .active_operation_id
                        .as_ref()
                        .ok_or_else(|| {
                            CoreError::invalid("assistant discovery has no active operation")
                        })?
                        .clone();
                    self.persist_operation_completion(
                        snapshot,
                        &operation_id,
                        &mut draft,
                        ProviderDiscoveryAction::AssistantRequestedMoreEvidence { question_count },
                        DurableOperationOutcome::Succeeded,
                        Vec::new(),
                        Vec::new(),
                        DiscoveryJsonUpdate::Preserve,
                    )?;
                } else {
                    let checkpoint = assistant_checkpoint(state)?;
                    self.persist_assistant_checkpoint(snapshot, &draft, checkpoint)?;
                }
                Ok(action)
            }
            Err(error) => {
                match state {
                    AssistantState::AwaitingRetryConsent => {
                        self.persist_assistant_checkpoint(
                            snapshot,
                            &draft,
                            DiscoveryAssistantCheckpoint::AwaitingRetryConsent,
                        )?;
                    }
                    AssistantState::Failed => {
                        let operation_id = snapshot
                            .active_operation_id
                            .as_ref()
                            .ok_or_else(|| {
                                CoreError::invalid("assistant discovery has no active operation")
                            })?
                            .clone();
                        self.persist_operation_completion(
                            snapshot,
                            &operation_id,
                            &mut draft,
                            ProviderDiscoveryAction::Fail {
                                failure: DiscoveryFailure {
                                    code: "assistant_invalid_output".to_owned(),
                                    message_key: "provider.discovery.assistant_invalid_output"
                                        .to_owned(),
                                    recoverable: false,
                                },
                            },
                            DurableOperationOutcome::Failed,
                            Vec::new(),
                            Vec::new(),
                            DiscoveryJsonUpdate::Preserve,
                        )?;
                    }
                    _ => {}
                }
                Err(assistant_error(error))
            }
        }
    }

    pub fn submit_assistant_tool_result(
        &self,
        session_id: &DiscoverySessionId,
        call_id: u64,
        result: AssistantToolResult,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        engine
            .submit_tool_result(call_id, result)
            .map_err(assistant_error)?;
        synchronize_assistant_snapshot(&mut draft, &engine);
        self.persist_assistant_checkpoint(&snapshot, &draft, DiscoveryAssistantCheckpoint::Ready)?;
        self.get(session_id)
    }

    /// Resumes one already-checkpointed Core-owned typed tool action.
    ///
    /// No model call is made and no native-provided tool payload is accepted.
    /// Every tool remains session-scoped and allowlisted by
    /// [`Self::execute_assistant_tool`], so a crash between execution and the
    /// checkpoint can safely repeat this idempotent read-only action.
    pub fn resume_assistant_core_host_action(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::BuildingAssistantManifestDraft {
            return Err(CoreError::invalid(
                "provider discovery is not running the setup assistant",
            ));
        }
        let draft = hydrate_working_draft(&snapshot)?;
        let engine = restored_assistant(&draft)?;
        let (call_id, call) = engine.pending_core_tool_call().map_err(assistant_error)?;
        let result = self.execute_assistant_tool(session_id, &call)?;
        self.submit_assistant_tool_result(session_id, call_id, result)
    }

    pub fn approve_assistant_retry(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        engine
            .approve_retry(session_id, true)
            .map_err(assistant_error)?;
        synchronize_assistant_snapshot(&mut draft, &engine);
        self.persist_assistant_checkpoint(&snapshot, &draft, DiscoveryAssistantCheckpoint::Ready)?;
        self.get(session_id)
    }

    pub fn request_assistant_draft_revision(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        engine.request_draft_revision().map_err(assistant_error)?;
        synchronize_assistant_snapshot(&mut draft, &engine);
        self.persist_assistant_checkpoint(
            &snapshot,
            &draft,
            DiscoveryAssistantCheckpoint::AwaitingRetryConsent,
        )?;
        self.get(session_id)
    }

    pub fn accept_assistant_draft(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let operation_id = snapshot
            .active_operation_id
            .as_ref()
            .ok_or_else(|| CoreError::invalid("assistant discovery has no active operation"))?
            .clone();
        let mut draft = hydrate_working_draft(&snapshot)?;
        let engine = restored_assistant(&draft)?;
        let review = engine
            .draft_review()
            .ok_or_else(|| CoreError::invalid("setup assistant has no draft to accept"))?;
        if !review.unresolved_conflicts.is_empty() || !review.draft.unresolved_questions.is_empty()
        {
            return Err(CoreError::invalid(
                "setup assistant draft still has unresolved conflicts or questions",
            ));
        }
        install_assistant_graph(&snapshot, &mut draft, &review.draft.manifest)?;
        draft.assistant_approval_binding = None;
        draft.assistant_more_evidence_questions.clear();
        let manifest_sha256 = validate_manifest(&review.draft.manifest)?
            .sha256()
            .to_owned();
        self.persist_operation_completion(
            &snapshot,
            &operation_id,
            &mut draft,
            ProviderDiscoveryAction::ManifestDraftBuilt { manifest_sha256 },
            DurableOperationOutcome::Succeeded,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )?;
        let (_cancel, cancelled) = watch::channel(false);
        self.drive_nonpersistent(session_id, None, cancelled)
    }

    pub fn record_assistant_failure(
        &self,
        session_id: &DiscoverySessionId,
        kind: AssistantFailureKind,
        retryable: bool,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        engine
            .record_failure(kind, retryable)
            .map_err(assistant_error)?;
        let state = engine.state();
        synchronize_assistant_snapshot(&mut draft, &engine);
        if state == AssistantState::AwaitingRetryConsent {
            self.persist_assistant_checkpoint(
                &snapshot,
                &draft,
                DiscoveryAssistantCheckpoint::AwaitingRetryConsent,
            )?;
        } else {
            let operation_id = snapshot
                .active_operation_id
                .as_ref()
                .ok_or_else(|| CoreError::invalid("assistant discovery has no active operation"))?;
            self.persist_operation_completion(
                &snapshot,
                operation_id,
                &mut draft,
                ProviderDiscoveryAction::Fail {
                    failure: DiscoveryFailure {
                        code: "assistant_failed".to_owned(),
                        message_key: "provider.discovery.assistant_failed".to_owned(),
                        recoverable: false,
                    },
                },
                DurableOperationOutcome::Failed,
                Vec::new(),
                Vec::new(),
                DiscoveryJsonUpdate::Preserve,
            )?;
        }
        self.get(session_id)
    }

    pub fn interrupt_assistant(
        &self,
        session_id: &DiscoverySessionId,
        outcome: DiscoveryInterruptionOutcome,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        engine.mark_interrupted().map_err(assistant_error)?;
        synchronize_assistant_snapshot(&mut draft, &engine);
        let operation_id = snapshot
            .active_operation_id
            .as_ref()
            .ok_or_else(|| CoreError::invalid("assistant discovery has no active operation"))?;
        let durable_outcome = match outcome {
            DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect => {
                DurableOperationOutcome::Interrupted
            }
            DiscoveryInterruptionOutcome::ExternalOutcomeUnknown => {
                DurableOperationOutcome::OutcomeUnknown
            }
        };
        self.persist_operation_completion(
            &snapshot,
            operation_id,
            &mut draft,
            ProviderDiscoveryAction::Interrupt {
                operation: DiscoveryOperationKind::BuildAssistantManifestDraft,
                outcome,
            },
            durable_outcome,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )?;
        self.get(session_id)
    }

    pub fn restart_assistant_after_interruption(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::Interrupted
            || !snapshot.session.recovery.as_ref().is_some_and(|recovery| {
                recovery.operation == DiscoveryOperationKind::BuildAssistantManifestDraft
            })
        {
            return Err(CoreError::invalid(
                "provider setup assistant is not explicitly restartable",
            ));
        }
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        if engine.state() != AssistantState::Interrupted {
            engine.mark_interrupted().map_err(assistant_error)?;
        }
        engine
            .restart_after_interruption(session_id, true)
            .map_err(assistant_error)?;
        synchronize_assistant_snapshot(&mut draft, &engine);
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::RestartInterrupted,
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        self.storage
            .persist_discovery_transition(&DiscoveryTransitionWrite {
                new_operation_id: Some(DiscoveryOperationId::new()),
                transition,
                draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
                review: DiscoveryJsonUpdate::Preserve,
                new_evidence: Vec::new(),
                new_candidates: Vec::new(),
                approval: None,
                completed_operation: None,
                prepared_commit: None,
                provider_graph: None,
                occurred_at: Utc::now(),
            })?;
        self.get(session_id)
    }

    fn persist_assistant_checkpoint(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        draft: &DiscoveryWorkingDraft,
        checkpoint: DiscoveryAssistantCheckpoint,
    ) -> CoreResult<()> {
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::AssistantCheckpointed { checkpoint },
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        self.storage
            .persist_discovery_transition(&DiscoveryTransitionWrite {
                transition,
                draft: DiscoveryJsonUpdate::Replace(working_draft_value(draft)?),
                review: DiscoveryJsonUpdate::Preserve,
                new_evidence: Vec::new(),
                new_candidates: Vec::new(),
                approval: None,
                new_operation_id: None,
                completed_operation: None,
                prepared_commit: None,
                provider_graph: None,
                occurred_at: Utc::now(),
            })?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn execute_nonpersistent_effect(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        operation: DiscoveryOperationKind,
        draft: &mut DiscoveryWorkingDraft,
        credential: Option<&str>,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<EffectCompletion> {
        match operation {
            DiscoveryOperationKind::ResolveKnownProvider => {
                if draft.deterministic.is_none() {
                    let site_intent = matches!(&draft.source, DiscoverySourceIntent::Site);
                    let source = match &draft.source {
                        DiscoverySourceIntent::KnownProvider { template_id } => {
                            DeterministicDiscoverySource::known_provider_id(template_id.clone())
                        }
                        DiscoverySourceIntent::Site => {
                            match DeterministicDiscoverySource::known_provider_site_with_policy(
                                snapshot.session.input.site_url.as_str(),
                                discovery_url_policy(&snapshot.session.input.connection_options)?,
                            ) {
                                Ok(source) => source,
                                Err(error) => return Err(deterministic_error(error)),
                            }
                        }
                        DiscoverySourceIntent::Curl => {
                            return Err(CoreError::invalid(
                                "sanitized cURL evidence must be supplied again after interruption",
                            ));
                        }
                    };
                    let active_templates = active_discovery_templates(self.storage)?;
                    let output = self.runtime.block_on(
                        DeterministicDiscoveryExecutor::new()
                            .execute_with_templates(source, &active_templates),
                    );
                    draft.deterministic = match output {
                        Ok(output) => Some(output),
                        Err(error)
                            if site_intent
                                && error.kind()
                                    == DeterministicDiscoveryErrorKind::KnownProviderNotFound =>
                        {
                            return Ok(EffectCompletion::simple(
                                ProviderDiscoveryAction::KnownProviderCandidatesResolved {
                                    candidate_count: 0,
                                },
                            ));
                        }
                        Err(error) => return Err(deterministic_error(error)),
                    };
                }
                let (evidence, candidates) =
                    deterministic_artifacts(snapshot, draft.deterministic.as_ref().expect("set"))?;
                let deterministic = draft.deterministic.clone().expect("set");
                record_deterministic_assistant_claims(snapshot, &deterministic, draft)?;
                draft.evidence_ids = evidence.iter().map(|record| record.id.clone()).collect();
                let candidate_count = u32::try_from(candidates.len())
                    .map_err(|_| CoreError::invalid("too many discovery candidates"))?;
                Ok(EffectCompletion {
                    action: ProviderDiscoveryAction::KnownProviderCandidatesResolved {
                        candidate_count,
                    },
                    evidence,
                    candidates,
                    review: DiscoveryJsonUpdate::Preserve,
                    outcome: DurableOperationOutcome::Succeeded,
                })
            }
            DiscoveryOperationKind::FetchDocuments => {
                let mut source = DeterministicDiscoverySource::site_with_policy(
                    snapshot.session.input.site_url.as_str(),
                    discovery_url_policy(&snapshot.session.input.connection_options)?,
                    DiscoveryFetchBudget::default(),
                )
                .map_err(deterministic_error)?;
                if let Some(docs_url) = &snapshot.session.input.docs_url {
                    source
                        .allow_document_url(docs_url.as_str())
                        .map_err(deterministic_error)?;
                }
                let output = self
                    .runtime
                    .block_on(DeterministicDiscoveryExecutor::new().execute(source))
                    .map_err(deterministic_error)?;
                draft.deterministic = Some(output);
                let (evidence, _) =
                    deterministic_artifacts(snapshot, draft.deterministic.as_ref().expect("set"))?;
                let deterministic = draft.deterministic.clone().expect("set");
                record_deterministic_assistant_claims(snapshot, &deterministic, draft)?;
                draft.evidence_ids = evidence.iter().map(|record| record.id.clone()).collect();
                let evidence_count = u32::try_from(evidence.len())
                    .map_err(|_| CoreError::invalid("too much discovery evidence"))?;
                Ok(EffectCompletion {
                    action: ProviderDiscoveryAction::DocumentsFetched { evidence_count },
                    evidence,
                    candidates: Vec::new(),
                    review: DiscoveryJsonUpdate::Preserve,
                    outcome: DurableOperationOutcome::Succeeded,
                })
            }
            DiscoveryOperationKind::ExtractEvidence => {
                let deterministic = draft.deterministic.as_ref();
                let has_deterministic_draft = deterministic.is_some_and(|output| {
                    !output.manifest_candidates.is_empty()
                        && (snapshot.session.input.preferred_assistant.is_none()
                            || output.manifest_candidates.iter().any(|candidate| {
                                candidate.confidence
                                    == DiscoveryCandidateConfidence::ExactCompiledProvider
                            }))
                });
                if has_deterministic_draft {
                    draft.assistant = None;
                    draft.assistant_approval_binding = None;
                    draft.assistant_more_evidence_questions.clear();
                    return Ok(EffectCompletion::simple(
                        ProviderDiscoveryAction::EvidenceExtracted {
                            resolution: DiscoveryEvidenceResolution::DeterministicDraftAvailable,
                        },
                    ));
                }
                if draft.assistant.is_some()
                    && restored_assistant(draft)?.state() == AssistantState::Ready
                {
                    let approval = draft.assistant_approval_binding.as_ref().ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::StorageCorrupted,
                            "resumable setup assistant lost its approval binding",
                            false,
                        )
                    })?;
                    return Ok(EffectCompletion::simple(
                        ProviderDiscoveryAction::AssistantResumedWithEvidence {
                            approval_id: approval.approval_id.clone(),
                            approval_grant_sha256: approval.grant_sha256.clone(),
                        },
                    ));
                }
                let resolution = if snapshot.session.input.preferred_assistant.is_some()
                    && !draft.evidence_ids.is_empty()
                {
                    initialize_assistant(self.storage, snapshot, draft)?;
                    DiscoveryEvidenceResolution::AssistantRecommended
                } else {
                    DiscoveryEvidenceResolution::MoreEvidenceRequired
                };
                Ok(EffectCompletion::simple(
                    ProviderDiscoveryAction::EvidenceExtracted { resolution },
                ))
            }
            DiscoveryOperationKind::BuildDeterministicManifestDraft => {
                build_deterministic_graph(self.storage, snapshot, draft, Utc::now())?;
                let template = draft
                    .template
                    .as_ref()
                    .ok_or_else(|| CoreError::internal("manifest build produced no template"))?;
                let manifest_sha256 = validate_manifest(&template.default_manifest)?
                    .sha256()
                    .to_owned();
                Ok(EffectCompletion::simple(
                    ProviderDiscoveryAction::ManifestDraftBuilt { manifest_sha256 },
                ))
            }
            DiscoveryOperationKind::ValidateManifest => {
                let template = draft
                    .template
                    .as_ref()
                    .ok_or_else(|| CoreError::internal("manifest validation has no template"))?;
                validate_connection_fields(&template.connection_fields)?;
                let validated = validate_manifest(&template.default_manifest)?;
                let connection = draft
                    .connection
                    .as_ref()
                    .ok_or_else(|| CoreError::internal("manifest validation has no connection"))?;
                let credential_required = template.default_manifest.auth != AuthBinding::None;
                if credential_required && connection.credential_ref.is_none() {
                    return Err(CoreError::invalid(
                        "authenticated provider discovery requires an opaque credential reference",
                    ));
                }
                Ok(EffectCompletion::simple(
                    ProviderDiscoveryAction::ManifestValidated {
                        manifest_sha256: validated.sha256().to_owned(),
                        credential_origin_approval_required: credential_required,
                    },
                ))
            }
            DiscoveryOperationKind::ListModels => {
                revalidate_discovery_catalog_authority(self.storage, draft, Utc::now())?;
                list_models_for_draft(self.runtime, snapshot, draft, credential, cancelled)?;
                let model_count = u32::try_from(draft.routes.len())
                    .map_err(|_| CoreError::invalid("too many listed models"))?;
                draft.probe_route_ids = draft.routes.iter().map(|route| route.id.clone()).collect();
                let probe_candidate_count = model_count;
                let review = if probe_candidate_count == 0 {
                    DiscoveryJsonUpdate::Replace(build_review(draft)?)
                } else {
                    DiscoveryJsonUpdate::Preserve
                };
                Ok(EffectCompletion {
                    action: ProviderDiscoveryAction::ModelsListed {
                        model_count,
                        probe_candidate_count,
                    },
                    evidence: Vec::new(),
                    candidates: model_candidates(snapshot, draft)?,
                    review,
                    outcome: DurableOperationOutcome::Succeeded,
                })
            }
            DiscoveryOperationKind::ProbeCapabilities => {
                revalidate_discovery_catalog_authority(self.storage, draft, Utc::now())?;
                let budget = approved_probe_budget(self.storage, snapshot, draft)?;
                let outcome =
                    probe_draft(self.runtime, snapshot, draft, credential, budget, cancelled)?;
                match outcome {
                    ProbeExecution::Completed { evidence } => Ok(EffectCompletion {
                        action: ProviderDiscoveryAction::ProbesCompleted,
                        evidence,
                        candidates: Vec::new(),
                        review: DiscoveryJsonUpdate::Replace(build_review(draft)?),
                        outcome: DurableOperationOutcome::Succeeded,
                    }),
                    ProbeExecution::Unknown => Ok(EffectCompletion {
                        action: ProviderDiscoveryAction::Interrupt {
                            operation,
                            outcome: DiscoveryInterruptionOutcome::ExternalOutcomeUnknown,
                        },
                        evidence: Vec::new(),
                        candidates: Vec::new(),
                        review: DiscoveryJsonUpdate::Preserve,
                        outcome: DurableOperationOutcome::OutcomeUnknown,
                    }),
                }
            }
            DiscoveryOperationKind::BuildAssistantManifestDraft
            | DiscoveryOperationKind::AtomicCommit
            | DiscoveryOperationKind::Compensation => Err(CoreError::invalid(
                "persistent or host-driven effect cannot run automatically",
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_native_no_effect_completion(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
        draft: &mut DiscoveryWorkingDraft,
        action: ProviderDiscoveryAction,
        outcome: DurableOperationOutcome,
        evidence: Vec<DiscoveryEvidenceRecord>,
        candidates: Vec<StoredDiscoveryCandidate>,
        review: DiscoveryJsonUpdate<DiscoveryReviewDiff>,
        context: &ProviderDiscoveryCredentialInstallContext,
    ) -> CoreResult<()> {
        let write = Self::operation_completion_write(
            snapshot,
            operation_id,
            draft,
            action,
            outcome,
            evidence,
            candidates,
            review,
        )?;
        let physical_authority_id = context.native_execution_id.clone().ok_or_else(|| {
            CoreError::invalid("native no-effect attestation has no started physical authority")
        })?;
        let attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
            context.operation_id.clone(),
            physical_authority_id,
            context.session_id.clone(),
            context.commit_attempt_id.clone(),
            context.commit_plan_sha256.clone(),
            context.connection_id.clone(),
        )?;
        self.storage
            .persist_native_no_effect_discovery_transition(&write, &attestation)?;
        Ok(())
    }
}

enum ProbeExecution {
    Completed {
        evidence: Vec<DiscoveryEvidenceRecord>,
    },
    Unknown,
}

pub(crate) fn resumable_assistant_operation_ids(
    storage: &Storage,
) -> CoreResult<BTreeSet<DiscoveryOperationId>> {
    let mut resumable = BTreeSet::new();
    for snapshot in storage.list_unfinished_discovery_sessions_for_recovery()? {
        if snapshot.session.state != DiscoveryState::BuildingAssistantManifestDraft {
            continue;
        }
        let operation_id = snapshot.active_operation_id.as_ref().ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "active setup assistant has no durable operation",
                false,
            )
        })?;
        let operation = storage
            .get_current_discovery_operation(&snapshot.session.id)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "active setup assistant operation is missing",
                    false,
                )
            })?;
        if operation.id != *operation_id
            || operation.kind != DiscoveryOperationKind::BuildAssistantManifestDraft
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "active setup assistant operation does not match its session",
                false,
            ));
        }
        let engine = restored_assistant(&hydrate_working_draft(&snapshot)?)?;
        if matches!(
            engine.state(),
            AssistantState::Ready
                | AssistantState::AwaitingToolResult
                | AssistantState::AwaitingRetryConsent
                | AssistantState::DraftReady
        ) {
            resumable.insert(operation_id.clone());
        }
    }
    Ok(resumable)
}

fn deterministic_id(session_id: &DiscoverySessionId, revision: u64, purpose: &str) -> String {
    Uuid::new_v5(
        &DISCOVERY_NAMESPACE,
        format!("{}\0{revision}\0{purpose}", session_id.as_str()).as_bytes(),
    )
    .to_string()
}

fn deterministic_action_id(
    session_id: &DiscoverySessionId,
    revision: u64,
    purpose: &str,
) -> DiscoveryActionId {
    DiscoveryActionId::parse(deterministic_id(session_id, revision, purpose))
        .expect("UUID is a valid discovery action id")
}

fn deterministic_commit_attempt_id(
    session_id: &DiscoverySessionId,
    revision: u64,
) -> DiscoveryCommitAttemptId {
    DiscoveryCommitAttemptId::parse(deterministic_id(session_id, revision, "commit-attempt"))
        .expect("UUID is a valid discovery commit id")
}

fn compensation_recipe(
    session_id: &DiscoverySessionId,
    revision: u64,
    plan: &DiscoveryCommitPlan,
) -> Vec<PreparedDiscoveryCompensationStep> {
    let mut steps = vec![PreparedDiscoveryCompensationStep {
        id: deterministic_id(session_id, revision, "compensation:restore-selection"),
        step: DiscoveryCompensationStep {
            action_id: deterministic_action_id(
                session_id,
                revision,
                "compensation:restore-selection",
            ),
            ordinal: 0,
            kind: DiscoveryCompensationKind::RestorePreviousSelection,
            target: DiscoveryCompensationTarget::RestorePreviousSelection {
                previous_selection: plan.previous_selection.clone(),
            },
            status: DiscoveryCompensationStatus::Pending,
        },
    }];
    steps.push(PreparedDiscoveryCompensationStep {
        id: deterministic_id(session_id, revision, "compensation:remove-graph"),
        step: DiscoveryCompensationStep {
            action_id: deterministic_action_id(session_id, revision, "compensation:remove-graph"),
            ordinal: 1,
            kind: DiscoveryCompensationKind::RemoveConnectionGraph,
            target: DiscoveryCompensationTarget::RemoveConnectionGraph {
                connection_id: plan.connection_id.clone(),
            },
            status: DiscoveryCompensationStatus::Pending,
        },
    });
    if let Some(credential_ref) = &plan.credential_ref {
        steps.push(PreparedDiscoveryCompensationStep {
            id: deterministic_id(session_id, revision, "compensation:remove-credential"),
            step: DiscoveryCompensationStep {
                action_id: deterministic_action_id(
                    session_id,
                    revision,
                    "compensation:remove-credential",
                ),
                ordinal: 2,
                kind: DiscoveryCompensationKind::RemoveCredentialSlot,
                target: DiscoveryCompensationTarget::RemoveCredentialSlot {
                    connection_id: plan.connection_id.clone(),
                    credential_ref: credential_ref.clone(),
                },
                status: DiscoveryCompensationStatus::Pending,
            },
        });
    }
    steps
}

fn canonical_serde_sha256<T: Serialize>(value: &T, label: &str) -> CoreResult<String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| CoreError::internal(format!("{label} could not be serialized")))?;
    Ok(sha256_hex(&bytes))
}

fn approval_proposal_for(
    session_id: &DiscoverySessionId,
    revision: u64,
    grant: DiscoveryApprovalGrant,
) -> CoreResult<ProviderDiscoveryApprovalProposal> {
    grant
        .validate()
        .map_err(|error| CoreError::invalid(format!("invalid discovery approval: {error}")))?;
    let grant_sha256 = canonical_serde_sha256(&grant, "discovery approval grant")?;
    let id = DiscoveryApprovalId::parse(deterministic_id(
        session_id,
        revision,
        &format!("approval:{grant_sha256}"),
    ))
    .map_err(|error| CoreError::internal(format!("approval id failed: {error}")))?;
    Ok(ProviderDiscoveryApprovalProposal {
        id,
        grant,
        grant_sha256,
    })
}

fn approval_record(
    snapshot: &DiscoverySessionSnapshot,
    proposal: ProviderDiscoveryApprovalProposal,
    decision: DiscoveryApprovalDecision,
    created_at: DateTime<Utc>,
) -> DiscoveryApprovalRecord {
    DiscoveryApprovalRecord {
        id: proposal.id,
        session_id: snapshot.session.id.clone(),
        session_revision: snapshot.session.revision,
        decision,
        grant: proposal.grant,
        created_at,
    }
}

fn require_approval_id(
    actual: &DiscoveryApprovalId,
    proposal: &ProviderDiscoveryApprovalProposal,
) -> CoreResult<()> {
    if actual != &proposal.id {
        return Err(CoreError::invalid(
            "discovery approval identifier does not match the current proposal",
        ));
    }
    Ok(())
}

fn require_approval_binding(
    actual_id: &DiscoveryApprovalId,
    actual_sha256: &str,
    proposal: &ProviderDiscoveryApprovalProposal,
) -> CoreResult<()> {
    require_approval_id(actual_id, proposal)?;
    if actual_sha256 != proposal.grant_sha256 {
        return Err(CoreError::invalid(
            "discovery approval hash does not match the exact typed grant",
        ));
    }
    Ok(())
}

fn credential_origin_proposal(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<ProviderDiscoveryApprovalProposal> {
    let grant = credential_origin_grant(snapshot, draft)?;
    approval_proposal_for(&snapshot.session.id, snapshot.session.revision, grant)
}

fn credential_origin_grant(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<DiscoveryApprovalGrant> {
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("credential proposal has no template"))?;
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("credential proposal has no connection"))?;
    let manifest_sha256 = snapshot
        .session
        .manifest_sha256
        .clone()
        .or_else(|| {
            validate_manifest(&template.default_manifest)
                .ok()
                .map(|validated| validated.sha256().to_owned())
        })
        .ok_or_else(|| CoreError::internal("credential proposal has no manifest hash"))?;
    Ok(DiscoveryApprovalGrant::CredentialOrigin {
        origin: connection.api_origin.clone(),
        auth_binding: template.default_manifest.auth.clone(),
        manifest_sha256,
    })
}

fn probe_proposal(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<ProviderDiscoveryApprovalProposal> {
    let mut route_ids = draft.probe_route_ids.clone();
    route_ids.sort();
    route_ids.dedup();
    let budget = standard_probe_budget(route_ids.len())?;
    approval_proposal_for(
        &snapshot.session.id,
        snapshot.session.revision,
        DiscoveryApprovalGrant::CapabilityProbe {
            model_route_ids: route_ids,
            budget,
        },
    )
}

fn approved_probe_budget(
    storage: &Storage,
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<DiscoveryProbeBudget> {
    let binding = snapshot
        .session
        .active_effect_approval
        .as_ref()
        .ok_or_else(|| CoreError::invalid("capability probe has no active approval binding"))?;
    let approval = storage
        .list_discovery_approvals(&snapshot.session.id, MAX_DISCOVERY_ROWS)?
        .into_iter()
        .find(|approval| approval.id == binding.approval_id)
        .ok_or_else(|| CoreError::invalid("capability probe approval record is missing"))?;
    if approval.decision != DiscoveryApprovalDecision::Approved
        || canonical_serde_sha256(&approval.grant, "capability probe approval grant")?
            != binding.grant_sha256
    {
        return Err(CoreError::invalid(
            "capability probe approval binding does not match its immutable grant",
        ));
    }
    let DiscoveryApprovalGrant::CapabilityProbe {
        model_route_ids,
        budget,
    } = approval.grant
    else {
        return Err(CoreError::invalid(
            "capability probe approval has the wrong grant type",
        ));
    };
    let mut expected_route_ids = draft.probe_route_ids.clone();
    expected_route_ids.sort();
    expected_route_ids.dedup();
    if model_route_ids != expected_route_ids
        || budget != standard_probe_budget(expected_route_ids.len())?
    {
        return Err(CoreError::invalid(
            "capability probe execution differs from the approved routes or budget",
        ));
    }
    Ok(budget)
}

fn assistant_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!(
        "provider setup assistant rejected the action: {error}"
    ))
}

fn assistant_structured_output_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        format!("provider setup assistant returned invalid structured output: {error}"),
        true,
    )
}

fn corrupted_assistant_resume_boundary() -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageCorrupted,
        "provider setup assistant recovery state is inconsistent",
        false,
    )
}

fn restored_assistant(draft: &DiscoveryWorkingDraft) -> CoreResult<SetupAssistantEngine> {
    let engine = SetupAssistantEngine::from_snapshot(
        draft
            .assistant
            .clone()
            .ok_or_else(|| CoreError::internal("setup assistant snapshot is missing"))?,
    )
    .map_err(|_| corrupted_assistant_resume_boundary())?;
    if engine.unresolved_questions() != draft.assistant_more_evidence_questions {
        return Err(corrupted_assistant_resume_boundary());
    }
    Ok(engine)
}

fn assistant_checkpoint(state: AssistantState) -> CoreResult<DiscoveryAssistantCheckpoint> {
    match state {
        AssistantState::Ready => Ok(DiscoveryAssistantCheckpoint::Ready),
        AssistantState::AwaitingAssistant => Ok(DiscoveryAssistantCheckpoint::AwaitingAssistant),
        AssistantState::AwaitingToolResult => Ok(DiscoveryAssistantCheckpoint::AwaitingToolResult),
        AssistantState::AwaitingMoreEvidence => {
            Ok(DiscoveryAssistantCheckpoint::AwaitingMoreEvidence)
        }
        AssistantState::AwaitingRetryConsent => {
            Ok(DiscoveryAssistantCheckpoint::AwaitingRetryConsent)
        }
        AssistantState::DraftReady => Ok(DiscoveryAssistantCheckpoint::DraftReady),
        AssistantState::AwaitingConsent
        | AssistantState::Interrupted
        | AssistantState::Failed
        | AssistantState::Cancelled => Err(CoreError::invalid(
            "setup assistant state cannot be checkpointed in the active operation",
        )),
    }
}

fn install_assistant_graph(
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    manifest: &ProviderManifest,
) -> CoreResult<()> {
    let mut manifest = manifest.clone();
    let api_base_path = snapshot
        .session
        .input
        .connection_options
        .api_base_path
        .as_ref()
        .or_else(|| {
            draft.deterministic.as_ref().and_then(|output| {
                output
                    .connection_hints
                    .iter()
                    .find(|hint| hint.api_family == manifest.api_family)
                    .and_then(|hint| hint.api_base_path.as_ref())
            })
        });
    embed_discovered_api_base_path(&mut manifest, api_base_path).map_err(deterministic_error)?;
    let validated = validate_manifest(&manifest)?;
    let manifest_sha256 = validated.sha256().to_owned();
    let connection_fields = AdapterRegistry::built_in_templates()?
        .into_iter()
        .find(|template| template.api_family == manifest.api_family)
        .map(|template| template.connection_fields)
        .unwrap_or_default();
    let template = ProviderTemplate {
        id: ProviderTemplateId::from(format!("discovered-{manifest_sha256}")),
        display_name: snapshot.session.input.display_name.clone(),
        manifest_version: 1,
        source: TemplateSource::UserDiscovered,
        api_family: manifest.api_family,
        connection_fields,
        default_manifest: manifest,
    };
    validate_connection_fields(&template.connection_fields)?;
    install_graph_seed_with_embedded_base(snapshot, draft, template, Utc::now())
}

#[allow(clippy::too_many_lines)]
async fn run_setup_assistant_provider_call(
    provider: Arc<dyn Provider>,
    route: &ModelRoute,
    prompt: &AssistantPromptPackage,
    estimate: AssistantCallEstimate,
    credential: Option<&str>,
) -> CoreResult<lorepia_providers::setup_assistant::AssistantTurn> {
    let conversation_id = ConversationId::new();
    let mut system = Message::user(
        conversation_id.clone(),
        prompt.system_instruction().to_owned(),
    );
    system.role = MessageRole::System;
    let untrusted_payload = prompt.untrusted_payload_json().map_err(assistant_error)?;
    let user = Message::user(conversation_id.clone(), untrusted_payload);
    let max_output_tokens = u32::try_from(estimate.maximum_output_tokens)
        .map_err(|_| CoreError::invalid("assistant output-token estimate is too large"))?;
    let request = GenerationRequest {
        generation_id: GenerationId::new(),
        conversation_id,
        model: route.model_id.clone(),
        messages: vec![system, user],
        temperature: None,
        max_output_tokens: Some(max_output_tokens),
        resolved_prompt_plan: None,
        provider_execution_plan_hash: None,
        provider_provenance: None,
        preserve_opaque_reasoning_state: false,
        opaque_reasoning_context: Vec::new(),
    };
    let output_limit = usize::try_from(estimate.maximum_output_tokens)
        .unwrap_or(usize::MAX)
        .saturating_mul(16)
        .clamp(1_024, 256 * 1024);
    let (sink, mut events) = mpsc::channel(32);
    let (_cancel_sender, cancel_receiver) = watch::channel(false);
    let request_plan = prompt.provider_request_plan(route.api_family);
    let generation = provider.generate_with_internal_plan(
        request,
        credential,
        sink,
        cancel_receiver,
        request_plan,
    );
    let collect = async move {
        let mut output = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                ProviderEvent::TextDelta(delta) => {
                    let next = output
                        .len()
                        .checked_add(delta.len())
                        .ok_or_else(|| CoreError::invalid("assistant output exceeded its bound"))?;
                    if next > output_limit {
                        return Err(CoreError::invalid(
                            "assistant output exceeded its bounded response size",
                        ));
                    }
                    output.extend_from_slice(delta.as_bytes());
                }
                ProviderEvent::ReasoningDelta(_) | ProviderEvent::OpaqueReasoningState(_) => {}
                ProviderEvent::ToolCallStarted { .. }
                | ProviderEvent::ToolCallArgumentsDelta { .. }
                | ProviderEvent::ToolCallCompleted { .. } => {
                    return Err(CoreError::invalid(
                        "provider-native tool calls are not allowed in setup assistant mode",
                    ));
                }
            }
        }
        if output.is_empty() {
            return Err(CoreError::invalid(
                "setup assistant returned an empty structured response",
            ));
        }
        Ok(output)
    };
    let (generation_result, output_result) = tokio::join!(generation, collect);
    if let Err(mut error) = generation_result {
        if let Ok(mut output) = output_result {
            output.zeroize();
        }
        let reflected = credential
            .filter(|value| !value.is_empty())
            .is_some_and(|credential| {
                error.message.contains(credential) || error.operation_id.contains(credential)
            });
        if reflected {
            error.message.zeroize();
            error.operation_id.zeroize();
            return Err(CoreError::new(
                CoreErrorCode::ProviderUnavailable,
                "setup assistant provider error reflected credential material",
                false,
            ));
        }
        return Err(error);
    }
    let mut output = output_result?;
    if credential
        .filter(|value| !value.is_empty())
        .is_some_and(|credential| {
            output
                .windows(credential.len())
                .any(|window| window == credential.as_bytes())
        })
    {
        output.zeroize();
        return Err(CoreError::new(
            CoreErrorCode::ProviderUnavailable,
            "setup assistant response reflected credential material",
            false,
        ));
    }
    let turn = match prompt.decode_schema_constrained_response(&output) {
        Ok(turn) => turn,
        Err(error) => {
            output.zeroize();
            return Err(assistant_structured_output_error(error));
        }
    };
    output.zeroize();
    Ok(turn)
}

const fn assistant_failure_kind(error: &CoreError) -> AssistantFailureKind {
    match error.code {
        CoreErrorCode::ProviderRateLimited => AssistantFailureKind::RateLimited,
        CoreErrorCode::NetworkUnavailable | CoreErrorCode::ProviderUnavailable => {
            AssistantFailureKind::Transport
        }
        CoreErrorCode::ProviderAuthFailed | CoreErrorCode::PermissionDenied => {
            AssistantFailureKind::ProviderRejected
        }
        CoreErrorCode::InvalidInput | CoreErrorCode::UnsupportedContent => {
            AssistantFailureKind::InvalidStructuredOutput
        }
        CoreErrorCode::Cancelled => AssistantFailureKind::Timeout,
        CoreErrorCode::UnsafeArchive
        | CoreErrorCode::NotFound
        | CoreErrorCode::StorageUnavailable
        | CoreErrorCode::StorageCorrupted
        | CoreErrorCode::Internal => AssistantFailureKind::Internal,
    }
}

fn capability_observation_support(observation: &CapabilityObservation) -> Option<bool> {
    match observation.status {
        SupportStatus::Unsupported => Some(false),
        SupportStatus::Unknown => None,
        SupportStatus::Verified
        | SupportStatus::Documented
        | SupportStatus::Inferred
        | SupportStatus::Conditional => match &observation.value {
            CapabilityValue::Boolean(value) => Some(*value),
            CapabilityValue::Integer(_)
            | CapabilityValue::EnumValues(_)
            | CapabilityValue::Structured(_) => Some(true),
        },
    }
}

const fn api_family_slug(family: ApiFamily) -> &'static str {
    match family {
        ApiFamily::OpenAiResponses => "openai_responses",
        ApiFamily::OpenAiChatCompletions => "openai_chat_completions",
        ApiFamily::AnthropicMessages => "anthropic_messages",
        ApiFamily::GeminiGenerateContent => "gemini_generate_content",
        ApiFamily::OllamaNative => "ollama_native",
    }
}

fn assistant_proposal(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<ProviderDiscoveryApprovalProposal> {
    let engine = SetupAssistantEngine::from_snapshot(
        draft
            .assistant
            .clone()
            .ok_or_else(|| CoreError::internal("assistant proposal has no durable snapshot"))?,
    )
    .map_err(assistant_error)?;
    let request = engine.consent_request().map_err(assistant_error)?;
    let mut evidence_ids = request.evidence_ids;
    evidence_ids.sort();
    evidence_ids.dedup();
    let mut allowed_document_origins = request
        .source_origins
        .into_iter()
        .map(|origin| {
            CanonicalOrigin::parse(&origin)
                .map_err(|error| CoreError::invalid(format!("invalid assistant origin: {error}")))
        })
        .collect::<CoreResult<Vec<_>>>()?;
    allowed_document_origins.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    allowed_document_origins.dedup();
    let max_input_tokens = u32::try_from(request.budget.max_input_tokens)
        .map_err(|_| CoreError::invalid("assistant input budget exceeds the approval contract"))?;
    let max_output_tokens = u32::try_from(request.budget.max_output_tokens)
        .map_err(|_| CoreError::invalid("assistant output budget exceeds the approval contract"))?;
    approval_proposal_for(
        &snapshot.session.id,
        snapshot.session.revision,
        DiscoveryApprovalGrant::AssistantConsent {
            assistant_route_id: request.assistant_route_id,
            evidence_ids,
            allowed_document_origins,
            max_calls: request.budget.max_turns,
            max_input_tokens,
            max_output_tokens,
            max_tool_calls: request.budget.max_tool_calls,
            max_retries: request.budget.max_retries,
            max_cost_micro_units: request.budget.max_cost_micro_units,
        },
    )
}

fn grant_assistant_snapshot(
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    grant: &DiscoveryApprovalGrant,
) -> CoreResult<()> {
    let DiscoveryApprovalGrant::AssistantConsent {
        assistant_route_id,
        evidence_ids,
        allowed_document_origins,
        ..
    } = grant
    else {
        return Err(CoreError::internal(
            "assistant approval used a non-assistant grant",
        ));
    };
    let mut engine = restored_assistant(draft)?;
    engine
        .grant_consent(AssistantConsent {
            session_id: snapshot.session.id.clone(),
            assistant_route_id: assistant_route_id.clone(),
            approved_evidence_ids: evidence_ids.clone(),
            approved_source_origins: allowed_document_origins
                .iter()
                .map(|origin| origin.as_str().to_owned())
                .collect(),
            allow_document_egress: true,
        })
        .map_err(assistant_error)?;
    synchronize_assistant_snapshot(draft, &engine);
    Ok(())
}

fn synchronize_assistant_snapshot(
    draft: &mut DiscoveryWorkingDraft,
    engine: &SetupAssistantEngine,
) {
    draft.assistant_more_evidence_questions = engine.unresolved_questions().to_vec();
    draft.assistant = Some(engine.snapshot());
}

fn cancel_assistant_snapshot(draft: &mut DiscoveryWorkingDraft) -> CoreResult<()> {
    if draft.assistant.is_none() {
        draft.assistant_more_evidence_questions.clear();
        return Ok(());
    }
    let mut engine = restored_assistant(draft)?;
    if !matches!(
        engine.state(),
        AssistantState::DraftReady | AssistantState::Failed | AssistantState::Cancelled
    ) {
        engine.cancel().map_err(assistant_error)?;
    }
    synchronize_assistant_snapshot(draft, &engine);
    Ok(())
}

fn initialize_assistant(
    storage: &Storage,
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
) -> CoreResult<()> {
    if draft.assistant.is_some() {
        restored_assistant(draft)?;
        return Ok(());
    }
    let assistant_route_id = snapshot
        .session
        .input
        .preferred_assistant
        .clone()
        .ok_or_else(|| CoreError::invalid("provider setup assistant route was not selected"))?;
    let wanted_ids = draft
        .evidence_ids
        .iter()
        .chain(&draft.extra_evidence_ids)
        .cloned()
        .collect::<BTreeSet<_>>();
    if wanted_ids.is_empty() {
        return Err(CoreError::invalid(
            "provider setup assistant requires redacted evidence",
        ));
    }
    let records = storage.list_discovery_evidence(&snapshot.session.id, MAX_DISCOVERY_ROWS)?;
    let evidence = records
        .into_iter()
        .filter(|record| wanted_ids.contains(&record.id))
        .map(|record| {
            let claims = draft
                .assistant_evidence_claims
                .get(&record.id)
                .cloned()
                .unwrap_or_default();
            redacted_assistant_evidence(record, claims)
        })
        .collect::<CoreResult<Vec<_>>>()?;
    if evidence.len() != wanted_ids.len() {
        return Err(CoreError::invalid(
            "provider setup assistant evidence is incomplete",
        ));
    }
    let mut allowed_api_families = draft
        .deterministic
        .as_ref()
        .map(|output| {
            output
                .family_candidates
                .iter()
                .map(|candidate| candidate.api_family)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if allowed_api_families.is_empty() {
        allowed_api_families = AdapterRegistry::built_in_templates()?
            .into_iter()
            .map(|template| template.api_family)
            .collect();
    }
    let mut engine = SetupAssistantEngine::new(
        snapshot.session.id.clone(),
        assistant_route_id,
        allowed_api_families,
        evidence,
        AssistantBudget::default(),
    )
    .map_err(assistant_error)?;
    if !draft.assistant_more_evidence_questions.is_empty() {
        let durable_questions = draft.assistant_more_evidence_questions.clone();
        engine
            .replace_unresolved_questions_before_consent(durable_questions.clone())
            .map_err(assistant_error)?;
        if engine.unresolved_questions() != durable_questions {
            return Err(corrupted_assistant_resume_boundary());
        }
    }
    synchronize_assistant_snapshot(draft, &engine);
    draft.assistant_approval_binding = None;
    Ok(())
}

fn redacted_assistant_evidence(
    record: DiscoveryEvidenceRecord,
    claims: Vec<EvidenceClaim>,
) -> CoreResult<RedactedAssistantEvidence> {
    let kind = match record.kind {
        DiscoveryEvidenceKind::OpenApi | DiscoveryEvidenceKind::JsonSchema => {
            AssistantEvidenceKind::ApiSpecification
        }
        DiscoveryEvidenceKind::JsonDocument => AssistantEvidenceKind::DeterministicExtraction,
        DiscoveryEvidenceKind::HtmlDocument
        | DiscoveryEvidenceKind::YamlDocument
        | DiscoveryEvidenceKind::XmlDocument
        | DiscoveryEvidenceKind::PlainTextDocument => AssistantEvidenceKind::OfficialDocument,
    };
    let excerpt_value = assistant_evidence_excerpt_value(&record.extracted_json);
    let excerpt = bounded_utf8_prefix(
        &serde_json::to_string(&excerpt_value)
            .map_err(|_| CoreError::internal("redacted assistant evidence could not be encoded"))?,
        16 * 1024,
    );
    RedactedAssistantEvidence::new(
        record.id,
        kind,
        record.source_url.as_str(),
        record.content_sha256,
        excerpt,
        claims,
        1,
    )
    .map_err(assistant_error)
}

fn assistant_evidence_excerpt_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(assistant_evidence_excerpt_value)
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .filter(|(name, _)| {
                    !matches!(
                        name.as_str(),
                        "content_sha256"
                            | "manifest_sha256"
                            | "path_sha256"
                            | "source_path_sha256"
                            | "template_id"
                    )
                })
                .map(|(name, value)| (name.clone(), assistant_evidence_excerpt_value(value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn bounded_utf8_prefix(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_owned()
}

fn proposal_for_state(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> Option<CoreResult<ProviderDiscoveryApprovalProposal>> {
    match snapshot.session.state {
        DiscoveryState::AwaitingCredentialOriginApproval => {
            Some(credential_origin_proposal(snapshot, draft))
        }
        DiscoveryState::AwaitingProbeConsent => Some(probe_proposal(snapshot, draft)),
        DiscoveryState::AwaitingAssistantConsent => Some(assistant_proposal(snapshot, draft)),
        DiscoveryState::AwaitingReview => {
            let review = snapshot.review.as_ref()?;
            Some(sanitized_graph_sha256(draft).and_then(|graph_sha256| {
                approval_proposal_for(
                    &snapshot.session.id,
                    snapshot.session.revision,
                    DiscoveryApprovalGrant::Review {
                        review_sha256: review.sha256.clone(),
                        graph_sha256,
                    },
                )
            }))
        }
        _ => None,
    }
}

fn record_deterministic_assistant_claims(
    snapshot: &DiscoverySessionSnapshot,
    output: &DeterministicDiscoveryOutput,
    draft: &mut DiscoveryWorkingDraft,
) -> CoreResult<()> {
    for (index, item) in output.evidence.iter().enumerate() {
        let evidence_id = EvidenceId::from(deterministic_id(
            &snapshot.session.id,
            0,
            &format!("evidence:{index}:{}", item.content_sha256),
        ));
        let claims = deterministic_assistant_claims(output, index)?;
        if !claims.is_empty() {
            draft.assistant_evidence_claims.insert(evidence_id, claims);
        }
    }
    Ok(())
}

fn deterministic_assistant_claims(
    output: &DeterministicDiscoveryOutput,
    evidence_index: usize,
) -> CoreResult<Vec<EvidenceClaim>> {
    let mut projected = BTreeMap::<DraftField, BTreeSet<String>>::new();
    for family in output
        .family_candidates
        .iter()
        .filter(|candidate| candidate.evidence_indices.contains(&evidence_index))
        .map(|candidate| candidate.api_family)
    {
        projected
            .entry(DraftField::ApiFamily)
            .or_default()
            .insert(api_family_slug(family).to_owned());
    }
    for candidate in output
        .manifest_candidates
        .iter()
        .filter(|candidate| candidate.evidence_indices.contains(&evidence_index))
    {
        let manifest = &candidate.template.default_manifest;
        projected
            .entry(DraftField::ApiFamily)
            .or_default()
            .insert(api_family_slug(manifest.api_family).to_owned());
        if let Some(origin) = &manifest.default_api_origin {
            projected
                .entry(DraftField::DefaultApiOrigin)
                .or_default()
                .insert(origin.as_str().to_owned());
        }
        if candidate.auth_evidenced {
            projected.entry(DraftField::Auth).or_default().insert(
                serde_json::to_string(&manifest.auth)
                    .map_err(|_| CoreError::internal("assistant auth claim encoding failed"))?,
            );
        }
        if candidate.generation_endpoint_evidenced {
            projected
                .entry(DraftField::GenerateEndpoint)
                .or_default()
                .insert(endpoint_claim(
                    manifest.endpoints.generate.method,
                    manifest.endpoints.generate.path.as_str(),
                ));
            projected
                .entry(DraftField::ResponseDecoder)
                .or_default()
                .insert(decoder_slug(manifest.decoders.response).to_owned());
        }
        if candidate.model_endpoint_evidenced
            && let Some(endpoint) = &manifest.endpoints.models
        {
            projected
                .entry(DraftField::ModelsEndpoint)
                .or_default()
                .insert(endpoint_claim(endpoint.method, endpoint.path.as_str()));
        }
        if deterministic_evidence_supports_streaming(&output.evidence[evidence_index])
            && let Some(decoder) = manifest.decoders.streaming
        {
            projected
                .entry(DraftField::StreamingDecoder)
                .or_default()
                .insert(decoder_slug(decoder).to_owned());
        }
    }
    projected
        .into_iter()
        .filter_map(|(field, values)| {
            (values.len() == 1).then(|| {
                EvidenceClaim::new(field, values.into_iter().next().expect("one value"))
                    .map_err(assistant_error)
            })
        })
        .collect()
}

fn deterministic_evidence_supports_streaming(
    evidence: &crate::provider_discovery_deterministic::RedactedDiscoveryEvidenceRecord,
) -> bool {
    [
        Some(&evidence.extracted_json),
        evidence.extracted_json.get("extracted"),
    ]
    .into_iter()
    .flatten()
    .any(|value| {
        value.get("stream_hint").and_then(Value::as_bool) == Some(true)
            || value
                .get("streaming_media_types")
                .and_then(Value::as_array)
                .is_some_and(|types| !types.is_empty())
    })
}

fn endpoint_claim(method: HttpMethod, path: &str) -> String {
    let method = match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
    };
    format!("{method} {path}")
}

const fn decoder_slug(decoder: DecoderId) -> &'static str {
    match decoder {
        DecoderId::OpenAiJsonV1 => "open_ai_json_v1",
        DecoderId::OpenAiSseV1 => "open_ai_sse_v1",
        DecoderId::AnthropicJsonV1 => "anthropic_json_v1",
        DecoderId::AnthropicSseV1 => "anthropic_sse_v1",
        DecoderId::GeminiJsonV1 => "gemini_json_v1",
        DecoderId::GeminiSseV1 => "gemini_sse_v1",
        DecoderId::OllamaJsonV1 => "ollama_json_v1",
        DecoderId::OllamaJsonlV1 => "ollama_jsonl_v1",
    }
}

fn deterministic_artifacts(
    snapshot: &DiscoverySessionSnapshot,
    output: &DeterministicDiscoveryOutput,
) -> CoreResult<(Vec<DiscoveryEvidenceRecord>, Vec<StoredDiscoveryCandidate>)> {
    let evidence = output
        .evidence
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let id = EvidenceId::from(deterministic_id(
                &snapshot.session.id,
                0,
                &format!("evidence:{index}:{}", item.content_sha256),
            ));
            let source_url = HttpUrl::parse(item.source_origin.as_str())
                .map_err(|error| CoreError::invalid(format!("invalid evidence origin: {error}")))?;
            Ok(DiscoveryEvidenceRecord {
                id,
                session_id: snapshot.session.id.clone(),
                kind: storage_evidence_kind(&item.kind),
                source_url,
                content_sha256: item.content_sha256.clone(),
                extracted_json: item.extracted_json.clone(),
                fetched_at: snapshot.created_at,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let candidates = output
        .manifest_candidates
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let evidence_ids = item
                .evidence_indices
                .iter()
                .filter_map(|index| evidence.get(*index).map(|record| record.id.clone()))
                .collect();
            let candidate = DiscoveryCandidate {
                id: DiscoveryCandidateId::parse(deterministic_id(
                    &snapshot.session.id,
                    0,
                    &format!(
                        "template-candidate:{index}:{}:{}",
                        item.template.id.as_str(),
                        item.template.manifest_version
                    ),
                ))
                .map_err(|error| CoreError::internal(format!("candidate id failed: {error}")))?,
                session_id: snapshot.session.id.clone(),
                summary: DiscoveryCandidateSummary::ProviderTemplate {
                    template_id: item.template.id.clone(),
                    template_version: item.template.manifest_version,
                },
                evidence_ids,
                created_at: snapshot.created_at,
            };
            Ok(StoredDiscoveryCandidate {
                candidate,
                proposed_revision: snapshot.session.revision,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    Ok((evidence, candidates))
}

fn storage_evidence_kind(kind: &str) -> DiscoveryEvidenceKind {
    match kind {
        "html_document" => DiscoveryEvidenceKind::HtmlDocument,
        "json_document" | "sanitized_curl_request" | "built_in_template" => {
            DiscoveryEvidenceKind::JsonDocument
        }
        "yaml_document" => DiscoveryEvidenceKind::YamlDocument,
        "xml_document" => DiscoveryEvidenceKind::XmlDocument,
        "json_schema" => DiscoveryEvidenceKind::JsonSchema,
        "open_api" => DiscoveryEvidenceKind::OpenApi,
        _ => DiscoveryEvidenceKind::PlainTextDocument,
    }
}

fn select_candidate(
    storage: &Storage,
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    candidate_id: &DiscoveryCandidateId,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    let entry = storage
        .read_discovery_candidates(&snapshot.session.id, MAX_DISCOVERY_ROWS)?
        .into_iter()
        .find(|entry| entry.candidate.id == *candidate_id)
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider discovery candidate was not found",
                false,
            )
        })?;
    let DiscoveryCandidateSummary::ProviderTemplate {
        template_id,
        template_version,
    } = entry.candidate.summary
    else {
        return Err(CoreError::invalid(
            "selected discovery candidate is not a provider template",
        ));
    };
    let template = draft
        .deterministic
        .as_ref()
        .and_then(|output| {
            output
                .manifest_candidates
                .iter()
                .find(|item| {
                    item.template.id == template_id
                        && item.template.manifest_version == template_version
                })
                .map(|item| item.template.clone())
        })
        .or_else(|| {
            storage
                .get_provider_template(&template_id, template_version)
                .ok()
        })
        .ok_or_else(|| CoreError::internal("selected provider template cannot be hydrated"))?;
    draft.selected_candidate_id = Some(candidate_id.clone());
    let catalog_authority = current_discovery_catalog_authority(storage, &template, observed_at)?;
    install_graph_seed(snapshot, draft, template, observed_at)?;
    draft.catalog_authority = catalog_authority;
    Ok(())
}

fn build_deterministic_graph(
    storage: &Storage,
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    if draft.template.is_some() && draft.connection.is_some() {
        return Ok(());
    }
    let output = draft
        .deterministic
        .as_ref()
        .ok_or_else(|| CoreError::invalid("no deterministic provider result is available"))?;
    let template = output
        .selected_template
        .clone()
        .or_else(|| {
            (output.manifest_candidates.len() == 1)
                .then(|| output.manifest_candidates[0].template.clone())
        })
        .ok_or_else(|| CoreError::invalid("provider template selection is still ambiguous"))?;
    let catalog_authority = current_discovery_catalog_authority(storage, &template, observed_at)?;
    install_graph_seed(snapshot, draft, template, observed_at)?;
    draft.catalog_authority = catalog_authority;
    Ok(())
}

fn install_graph_seed(
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    template: ProviderTemplate,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    install_graph_seed_internal(snapshot, draft, template, observed_at, false)
}

fn install_graph_seed_with_embedded_base(
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    template: ProviderTemplate,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    install_graph_seed_internal(snapshot, draft, template, observed_at, true)
}

fn install_graph_seed_internal(
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    template: ProviderTemplate,
    observed_at: DateTime<Utc>,
    api_base_path_is_embedded: bool,
) -> CoreResult<()> {
    require_active_discovery_network_authority(
        &snapshot.session.input.connection_options,
        observed_at,
    )?;
    validate_connection_fields(&template.connection_fields)?;
    let hint = draft.deterministic.as_ref().and_then(|output| {
        output
            .connection_hints
            .iter()
            .find(|hint| hint.api_family == template.api_family)
    });
    let api_origin = hint
        .map(|hint| hint.api_origin.clone())
        .or_else(|| template.default_manifest.default_api_origin.clone())
        .or_else(|| origin_from_http_url(&snapshot.session.input.site_url).ok())
        .ok_or_else(|| CoreError::invalid("provider API origin could not be determined"))?;
    let options = &snapshot.session.input.connection_options;
    let template_owns_api_base_path =
        api_base_path_is_embedded || template.source == TemplateSource::UserDiscovered;
    if template_owns_api_base_path
        && let Some(explicit_base_path) = &options.api_base_path
        && !manifest_endpoints_include_base(&template.default_manifest, explicit_base_path)
    {
        return Err(CoreError::invalid(
            "explicit API base path conflicts with the self-contained discovered template",
        ));
    }
    let api_base_path = if template_owns_api_base_path {
        None
    } else {
        options
            .api_base_path
            .clone()
            .or_else(|| hint.and_then(|hint| hint.api_base_path.clone()))
    };
    let values = resolved_discovery_connection_values(
        &template,
        &options.values,
        &api_origin,
        api_base_path.as_ref(),
    )?;
    validate_discovery_connection_values(
        &template,
        &values,
        snapshot.session.input.credential_ref.as_ref(),
    )?;
    let local_network_approval = normalized_local_network_approval(options, &api_origin)?;
    let created_at = if options.network_mode == ProviderNetworkMode::ApprovedLocalNetwork {
        options.local_network_approved_at.ok_or_else(|| {
            CoreError::invalid(
                "legacy LAN discovery has no approval issue time; restart provider discovery",
            )
        })?
    } else {
        observed_at
    };
    draft.connection = Some(ProviderConnection {
        id: snapshot.session.input.connection_id.clone(),
        template_id: template.id.clone(),
        template_version: template.manifest_version,
        display_name: snapshot.session.input.display_name.clone(),
        api_origin,
        config: ConnectionConfig {
            api_base_path,
            network_mode: options.network_mode,
            local_network_approval,
            values,
        },
        credential_ref: snapshot.session.input.credential_ref.clone(),
        credential_scope: None,
        timeout_seconds: options.timeout_seconds,
        status: ConnectionStatus::Untested,
        created_at,
        updated_at: observed_at,
    });
    draft.template = Some(template);
    draft.catalog_authority = None;
    Ok(())
}

fn manifest_endpoints_include_base(
    manifest: &ProviderManifest,
    api_base_path: &lorepia_domain::EndpointPath,
) -> bool {
    let includes_base = |path: &lorepia_domain::EndpointPath| {
        let base = api_base_path.as_str().trim_end_matches('/');
        base.is_empty()
            || path.as_str() == base
            || path
                .as_str()
                .strip_prefix(base)
                .is_some_and(|remainder| remainder.starts_with('/'))
    };
    includes_base(&manifest.endpoints.generate.path)
        && manifest
            .endpoints
            .models
            .as_ref()
            .is_none_or(|endpoint| includes_base(&endpoint.path))
}

fn resolved_discovery_connection_values(
    template: &ProviderTemplate,
    supplied: &[lorepia_domain::ConnectionConfigEntry],
    api_origin: &CanonicalOrigin,
    api_base_path: Option<&lorepia_domain::EndpointPath>,
) -> CoreResult<Vec<lorepia_domain::ConnectionConfigEntry>> {
    let mut values = supplied.to_vec();
    let base_url_is_declared = template.connection_fields.iter().any(|field| {
        field.key.eq_ignore_ascii_case("api_base_url")
            && field.value_type == ConnectionFieldType::Text
    });
    let base_url_is_supplied = values
        .iter()
        .any(|entry| entry.key.eq_ignore_ascii_case("api_base_url"));
    if base_url_is_declared && !base_url_is_supplied {
        let mut value = api_origin.as_str().trim_end_matches('/').to_owned();
        if let Some(path) = api_base_path
            && path.as_str() != "/"
        {
            value.push('/');
            value.push_str(path.as_str().trim_start_matches('/'));
        }
        HttpUrl::parse(&value).map_err(|error| {
            CoreError::invalid(format!("derived API base URL is invalid: {error}"))
        })?;
        values.push(lorepia_domain::ConnectionConfigEntry {
            key: "api_base_url".to_owned(),
            value: ConnectionConfigValue::Text(value),
        });
    }
    Ok(values)
}

fn validate_discovery_connection_values(
    template: &ProviderTemplate,
    values: &[lorepia_domain::ConnectionConfigEntry],
    credential_ref: Option<&CredentialRef>,
) -> CoreResult<()> {
    let mut supplied = std::collections::BTreeMap::new();
    for entry in values {
        let normalized = entry.key.to_ascii_lowercase();
        if supplied.insert(normalized, &entry.value).is_some() {
            return Err(CoreError::invalid(
                "provider connection values contain duplicate keys",
            ));
        }
    }

    let mut declared = std::collections::BTreeSet::new();
    for field in &template.connection_fields {
        let normalized = field.key.to_ascii_lowercase();
        declared.insert(normalized.clone());
        let supplied_value = supplied.get(&normalized).copied();
        match field.value_type {
            ConnectionFieldType::Credential => {
                if supplied_value.is_some() {
                    return Err(CoreError::invalid(
                        "credential fields must use the native credential reference",
                    ));
                }
                if field.required && credential_ref.is_none() {
                    return Err(CoreError::invalid(
                        "provider connection is missing its required credential reference",
                    ));
                }
            }
            ConnectionFieldType::Text => {
                if supplied_value
                    .is_some_and(|value| !matches!(value, ConnectionConfigValue::Text(_)))
                {
                    return Err(CoreError::invalid(
                        "provider connection text field has the wrong value type",
                    ));
                }
                if field.required && supplied_value.is_none() {
                    return Err(CoreError::invalid(
                        "provider connection is missing a required text value",
                    ));
                }
            }
            ConnectionFieldType::Integer => {
                if supplied_value
                    .is_some_and(|value| !matches!(value, ConnectionConfigValue::Integer(_)))
                {
                    return Err(CoreError::invalid(
                        "provider connection integer field has the wrong value type",
                    ));
                }
                if field.required && supplied_value.is_none() {
                    return Err(CoreError::invalid(
                        "provider connection is missing a required integer value",
                    ));
                }
            }
            ConnectionFieldType::Boolean => {
                if supplied_value
                    .is_some_and(|value| !matches!(value, ConnectionConfigValue::Boolean(_)))
                {
                    return Err(CoreError::invalid(
                        "provider connection boolean field has the wrong value type",
                    ));
                }
                if field.required && supplied_value.is_none() {
                    return Err(CoreError::invalid(
                        "provider connection is missing a required boolean value",
                    ));
                }
            }
        }
    }
    if supplied.keys().any(|key| !declared.contains(key)) {
        return Err(CoreError::invalid(
            "provider connection contains a value not declared by its template",
        ));
    }
    Ok(())
}

fn normalized_local_network_approval(
    options: &ProviderDiscoveryConnectionOptions,
    api_origin: &CanonicalOrigin,
) -> CoreResult<Option<ProviderLocalNetworkApproval>> {
    match (
        options.network_mode,
        options.local_network_approval.as_ref(),
    ) {
        (ProviderNetworkMode::Public | ProviderNetworkMode::LocalLoopback, None) => Ok(None),
        (ProviderNetworkMode::ApprovedLocalNetwork, Some(approval)) => {
            if &approval.origin != api_origin {
                return Err(CoreError::invalid(
                    "local-network approval origin must exactly match the discovered API origin",
                ));
            }
            let approved =
                ApprovedLocalNetworkOrigin::new(approval.origin.as_str(), &approval.addresses)
                    .map_err(|error| {
                        CoreError::invalid(format!(
                            "provider local-network approval is invalid: {error}"
                        ))
                    })?;
            Ok(Some(ProviderLocalNetworkApproval {
                origin: api_origin.clone(),
                addresses: approved.addresses().to_vec(),
            }))
        }
        (ProviderNetworkMode::ApprovedLocalNetwork, None) => Err(CoreError::invalid(
            "approved local-network mode requires an exact origin and address approval",
        )),
        (ProviderNetworkMode::Public | ProviderNetworkMode::LocalLoopback, Some(_)) => {
            Err(CoreError::invalid(
                "local-network approval is valid only in approved local-network mode",
            ))
        }
    }
}

fn origin_from_http_url(url: &HttpUrl) -> CoreResult<CanonicalOrigin> {
    let parsed = url::Url::parse(url.as_str())
        .map_err(|_| CoreError::invalid("provider discovery URL is invalid"))?;
    let origin = parsed.origin().ascii_serialization();
    CanonicalOrigin::parse(&origin)
        .map_err(|error| CoreError::invalid(format!("provider origin is invalid: {error}")))
}

fn apply_credential_origin_scope(template: &ProviderTemplate, connection: &mut ProviderConnection) {
    connection.credential_scope = Some(CredentialScope {
        allowed_origins: vec![connection.api_origin.clone()],
        auth_binding: template.default_manifest.auth.clone(),
        redirect_policy: CredentialRedirectPolicy::Deny,
    });
}

fn canonical_discovery_credential_connection(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<ProviderConnection> {
    let template = draft
        .template
        .clone()
        .ok_or_else(|| CoreError::internal("credential lease has no template draft"))?;
    let mut canonical = DiscoveryWorkingDraft::new(draft.source.clone());
    canonical.deterministic.clone_from(&draft.deterministic);
    install_graph_seed(snapshot, &mut canonical, template, snapshot.created_at)?;
    canonical
        .connection
        .ok_or_else(|| CoreError::internal("credential lease connection could not be rebuilt"))
}

fn canonical_discovery_credential_binding_sha256(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<String> {
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("credential lease has no template draft"))?;
    let mut connection = canonical_discovery_credential_connection(snapshot, draft)?;
    apply_credential_origin_scope(template, &mut connection);
    lorepia_storage::provider_credential_binding_sha256_for_connection(&connection)
}

fn validated_discovery_credential_binding_sha256(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
    connection: &ProviderConnection,
) -> CoreResult<String> {
    let current = lorepia_storage::provider_credential_binding_sha256_for_connection(connection)?;
    if current != canonical_discovery_credential_binding_sha256(snapshot, draft)? {
        return Err(CoreError::invalid(
            "provider credential binding changed after origin approval",
        ));
    }
    Ok(current)
}

fn require_discovery_credential_reference(
    snapshot: &DiscoverySessionSnapshot,
    connection: &ProviderConnection,
) -> CoreResult<()> {
    let input_reference = snapshot
        .session
        .input
        .credential_ref
        .as_ref()
        .ok_or_else(|| CoreError::invalid("credential lease has no opaque credential reference"))?;
    if input_reference.as_str() != snapshot.session.input.connection_id.as_str()
        || connection.id != snapshot.session.input.connection_id
        || connection.credential_ref.as_ref() != Some(input_reference)
    {
        return Err(CoreError::invalid(
            "credential lease reference is detached from its discovery connection",
        ));
    }
    Ok(())
}

fn discovery_state_accepts_credential_lease(session: &ProviderDiscoverySession) -> bool {
    match session.state {
        DiscoveryState::AwaitingCredentialOriginApproval
        | DiscoveryState::ListingModels
        | DiscoveryState::AwaitingProbeConsent
        | DiscoveryState::ProbingCapabilities
        | DiscoveryState::AwaitingReview
        | DiscoveryState::Committing => true,
        DiscoveryState::Interrupted => session.recovery.as_ref().is_some_and(|checkpoint| {
            matches!(
                checkpoint.operation,
                DiscoveryOperationKind::ListModels | DiscoveryOperationKind::ProbeCapabilities
            )
        }),
        _ => false,
    }
}

fn approved_discovery_credential_origin_authority(
    storage: &Storage,
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<(DiscoveryApprovalId, String)> {
    let approval_id = draft
        .credential_approval_id
        .as_ref()
        .ok_or_else(|| CoreError::invalid("credential lease has no durable origin approval"))?;
    let approval = storage
        .list_discovery_approvals(&snapshot.session.id, MAX_DISCOVERY_ROWS)?
        .into_iter()
        .find(|approval| &approval.id == approval_id)
        .ok_or_else(|| CoreError::invalid("credential lease origin approval record is missing"))?;
    validate_credential_origin_approval(snapshot, draft, &approval)?;
    Ok((
        approval.id,
        canonical_serde_sha256(&approval.grant, "credential-origin approval grant")?,
    ))
}

fn validate_credential_origin_approval(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
    approval: &DiscoveryApprovalRecord,
) -> CoreResult<()> {
    if approval.session_id != snapshot.session.id
        || approval.decision != DiscoveryApprovalDecision::Approved
        || Some(&approval.id) != draft.credential_approval_id.as_ref()
    {
        return Err(CoreError::invalid(
            "credential lease origin approval is not valid for this session",
        ));
    }
    let expected_grant = credential_origin_grant(snapshot, draft)?;
    if approval.grant != expected_grant {
        return Err(CoreError::invalid(
            "credential lease differs from its approved origin or authentication binding",
        ));
    }
    let proposal = approval_proposal_for(
        &snapshot.session.id,
        approval.session_revision,
        approval.grant.clone(),
    )?;
    if proposal.id != approval.id {
        return Err(CoreError::invalid(
            "credential lease origin approval identifier is not canonical",
        ));
    }
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("credential lease has no template draft"))?;
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("credential lease has no connection draft"))?;
    let mut expected_connection = connection.clone();
    apply_credential_origin_scope(template, &mut expected_connection);
    if connection.credential_scope != expected_connection.credential_scope {
        return Err(CoreError::invalid(
            "provider credential scope changed after origin approval",
        ));
    }
    Ok(())
}

fn sanitized_graph_sha256(draft: &DiscoveryWorkingDraft) -> CoreResult<String> {
    let template = draft
        .template
        .clone()
        .ok_or_else(|| CoreError::internal("provider graph has no template"))?;
    let connection = draft
        .connection
        .clone()
        .ok_or_else(|| CoreError::internal("provider graph has no connection"))?;
    let placeholder_plan = DiscoveryCommitPlan {
        attempt_id: DiscoveryCommitAttemptId::parse("ownership-hash-placeholder")
            .map_err(|error| CoreError::internal(format!("placeholder id failed: {error}")))?,
        session_id: DiscoverySessionId::from("ownership-hash-placeholder"),
        expected_revision: 0,
        manifest_sha256: "0".repeat(64),
        graph_sha256: "0".repeat(64),
        template_id: template.id.clone(),
        template_version: template.manifest_version,
        connection_id: connection.id.clone(),
        model_route_ids: draft.routes.iter().map(|route| route.id.clone()).collect(),
        credential_ref: connection.credential_ref.clone(),
        credential_approval_id: draft.credential_approval_id.clone(),
        review_sha256: "0".repeat(64),
        catalog_authority: draft.catalog_authority.clone(),
        previous_selection: DiscoveryPreviousSelection::None,
    };
    DiscoveredProviderGraph {
        plan: placeholder_plan,
        plan_sha256: "0".repeat(64),
        template,
        connection,
        routes: draft.routes.clone(),
        observations: draft.observations.clone(),
        presets: draft.presets.clone(),
    }
    .ownership_sha256()
}

fn build_review(draft: &DiscoveryWorkingDraft) -> CoreResult<DiscoveryReviewDiff> {
    let graph_sha256 = sanitized_graph_sha256(draft)?;
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("review has no provider template"))?;
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("review has no provider connection"))?;
    let mut changes = vec![
        DiscoveryReviewChange {
            kind: DiscoveryReviewChangeKind::Add,
            target_kind: "provider_template".to_owned(),
            target_id: template.id.as_str().to_owned(),
            summary_key: "discovery.review.add_provider_template".to_owned(),
            evidence_ids: draft.evidence_ids.clone(),
        },
        DiscoveryReviewChange {
            kind: DiscoveryReviewChangeKind::Add,
            target_kind: "provider_connection".to_owned(),
            target_id: connection.id.as_str().to_owned(),
            summary_key: "discovery.review.add_provider_connection".to_owned(),
            evidence_ids: draft.evidence_ids.clone(),
        },
    ];
    changes.extend(draft.routes.iter().map(|route| DiscoveryReviewChange {
        kind: DiscoveryReviewChangeKind::Add,
        target_kind: "model_route".to_owned(),
        target_id: route.id.as_str().to_owned(),
        summary_key: "discovery.review.add_model_route".to_owned(),
        evidence_ids: Vec::new(),
    }));
    DiscoveryReviewDiff::new(graph_sha256, changes, 0, draft.probe_failure_count)
        .map_err(|error| CoreError::invalid(format!("invalid discovery review: {error}")))
}

fn commit_plan_for(
    storage: &Storage,
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
    attempt_id: DiscoveryCommitAttemptId,
    review: &DiscoveryReviewDiff,
) -> CoreResult<DiscoveryCommitPlan> {
    revalidate_discovery_catalog_authority(storage, draft, Utc::now())?;
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("commit plan has no template"))?;
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("commit plan has no connection"))?;
    let manifest_sha256 = validate_manifest(&template.default_manifest)?
        .sha256()
        .to_owned();
    let graph_sha256 = sanitized_graph_sha256(draft)?;
    if review.graph_sha256 != graph_sha256 {
        return Err(CoreError::invalid(
            "persisted review does not match the sanitized provider graph",
        ));
    }
    let plan = DiscoveryCommitPlan {
        attempt_id,
        session_id: snapshot.session.id.clone(),
        expected_revision: snapshot.session.revision,
        manifest_sha256,
        graph_sha256,
        template_id: template.id.clone(),
        template_version: template.manifest_version,
        connection_id: connection.id.clone(),
        model_route_ids: draft.routes.iter().map(|route| route.id.clone()).collect(),
        credential_ref: connection.credential_ref.clone(),
        credential_approval_id: draft.credential_approval_id.clone(),
        review_sha256: review.sha256.clone(),
        catalog_authority: draft.catalog_authority.clone(),
        previous_selection: storage.current_discovery_previous_selection()?,
    };
    plan.validate()
        .map_err(|error| CoreError::invalid(format!("invalid discovery commit plan: {error}")))?;
    Ok(plan)
}

fn graph_from_plan(
    draft: &DiscoveryWorkingDraft,
    plan: DiscoveryCommitPlan,
    plan_sha256: String,
) -> CoreResult<DiscoveredProviderGraph> {
    let graph = DiscoveredProviderGraph {
        plan,
        plan_sha256,
        template: draft
            .template
            .clone()
            .ok_or_else(|| CoreError::internal("commit graph has no template"))?,
        connection: draft
            .connection
            .clone()
            .ok_or_else(|| CoreError::internal("commit graph has no connection"))?,
        routes: draft.routes.clone(),
        observations: draft.observations.clone(),
        presets: draft.presets.clone(),
    };
    if graph.ownership_sha256()? != graph.plan.graph_sha256 {
        return Err(CoreError::invalid(
            "provider graph changed after review approval",
        ));
    }
    Ok(graph)
}

const STANDARD_DISCOVERY_PROBE_PLAN: [CapabilityProbeKind;
    DiscoveryProbeBudget::PROBES_PER_ROUTE as usize] = [
    CapabilityProbeKind::Streaming,
    CapabilityProbeKind::Reasoning,
    CapabilityProbeKind::StructuredOutput,
    CapabilityProbeKind::ToolCalling,
    CapabilityProbeKind::PromptCaching,
];

fn standard_probe_budget(route_count: usize) -> CoreResult<DiscoveryProbeBudget> {
    DiscoveryProbeBudget::standard_for_plan(route_count, STANDARD_DISCOVERY_PROBE_PLAN.len())
        .map_err(|error| CoreError::invalid(format!("invalid capability probe budget: {error}")))
}

fn approved_probe_routes(
    draft: &DiscoveryWorkingDraft,
    approved_budget: DiscoveryProbeBudget,
) -> CoreResult<Vec<ModelRoute>> {
    if approved_budget != standard_probe_budget(draft.probe_route_ids.len())? {
        return Err(CoreError::invalid(
            "capability probe budget does not match the exact approved route set",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut routes = Vec::with_capacity(draft.probe_route_ids.len());
    for route_id in &draft.probe_route_ids {
        if !seen.insert(route_id.clone()) {
            return Err(CoreError::invalid(
                "capability probe route set contains a duplicate route",
            ));
        }
        let mut matches = draft.routes.iter().filter(|route| route.id == *route_id);
        let route = matches.next().ok_or_else(|| {
            CoreError::invalid("capability probe route is outside the approved working graph")
        })?;
        if matches.next().is_some() {
            return Err(CoreError::invalid(
                "capability probe working graph contains a duplicate route",
            ));
        }
        routes.push(route.clone());
    }
    Ok(routes)
}

#[allow(clippy::too_many_lines)]
fn probe_draft(
    runtime: &Handle,
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    credential: Option<&str>,
    approved_budget: DiscoveryProbeBudget,
    cancelled: watch::Receiver<bool>,
) -> CoreResult<ProbeExecution> {
    let approved_routes = approved_probe_routes(draft, approved_budget)?;
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("capability probe has no template"))?;
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("capability probe has no connection"))?;
    let budget = ProbeBudget::new(
        approved_budget.max_total_tokens_per_request,
        approved_budget.max_output_tokens_per_request,
        approved_budget.max_cost_micro_usd_per_request,
        Duration::from_millis(approved_budget.max_duration_millis_per_request),
        approved_budget.max_calls_per_request,
    )?;
    let registry = AdapterRegistry::new();
    let evidence_source_url = HttpUrl::parse(connection.api_origin.as_str())
        .map_err(|error| CoreError::invalid(format!("invalid probe evidence origin: {error}")))?;
    let engine = CapabilityProbeEngine::new();
    let mut request_count = 0_u32;
    let mut evidence = Vec::new();
    for route in approved_routes {
        if *cancelled.borrow() {
            return if request_count == 0 {
                Err(CoreError::new(
                    CoreErrorCode::Cancelled,
                    "provider discovery was cancelled before capability probing started",
                    false,
                ))
            } else {
                Ok(ProbeExecution::Unknown)
            };
        }
        let provider =
            registry.build_provider_for_route_with_plan(template, connection, &route, None)?;
        for probe in STANDARD_DISCOVERY_PROBE_PLAN {
            if *cancelled.borrow() {
                return if request_count == 0 {
                    Err(CoreError::new(
                        CoreErrorCode::Cancelled,
                        "provider discovery was cancelled before capability probing started",
                        false,
                    ))
                } else {
                    Ok(ProbeExecution::Unknown)
                };
            }
            request_count = request_count
                .checked_add(1)
                .ok_or_else(|| CoreError::invalid("capability probe request count overflowed"))?;
            if request_count > approved_budget.max_requests {
                return Err(CoreError::invalid(
                    "capability probe execution exceeds the approved request count",
                ));
            }
            let Ok(adapter) = ProviderCapabilityProbeAdapter::new(
                route.api_family,
                route.id.clone(),
                route.model_id.clone(),
                Arc::clone(&provider),
                credential,
                probe,
                approved_budget.max_cost_micro_usd_per_request,
            ) else {
                draft.probe_failure_count = draft.probe_failure_count.saturating_add(1);
                continue;
            };
            let consent_id = deterministic_id(
                &snapshot.session.id,
                snapshot.session.revision,
                &format!("probe:{}:{}", route.id.as_str(), probe_slug(probe)),
            );
            let consent = ProbeConsent::new(consent_id, route.id.clone(), probe, budget)?;
            match runtime.block_on(engine.run(
                Arc::new(adapter),
                &route.id,
                probe,
                consent,
                cancelled.clone(),
            )) {
                ProbeRunOutcome::Observed(observation) => {
                    evidence.push(capability_probe_evidence(
                        snapshot,
                        &evidence_source_url,
                        &observation,
                    )?);
                    draft.observations.push(observation);
                }
                ProbeRunOutcome::Failed(_) | ProbeRunOutcome::CancelledBeforeStart => {
                    draft.probe_failure_count = draft.probe_failure_count.saturating_add(1);
                }
                ProbeRunOutcome::UnknownOutcome(_) => return Ok(ProbeExecution::Unknown),
            }
        }
    }
    if request_count != approved_budget.max_requests {
        return Err(CoreError::invalid(
            "capability probe execution did not match the approved request count",
        ));
    }
    Ok(ProbeExecution::Completed { evidence })
}

fn capability_probe_evidence(
    snapshot: &DiscoverySessionSnapshot,
    source_url: &HttpUrl,
    observation: &CapabilityObservation,
) -> CoreResult<DiscoveryEvidenceRecord> {
    let id = observation.evidence_ref.clone().ok_or_else(|| {
        CoreError::internal("capability probe observation has no evidence reference")
    })?;
    let extracted_json = serde_json::json!({
        "kind": "capability_probe",
        "model_route_id": observation.model_route_id,
        "capability": observation.key,
        "value": observation.value,
        "status": observation.status,
        "source": observation.source,
        "confidence": observation.confidence,
        "observed_at": observation.observed_at,
        "expires_at": observation.expires_at,
    });
    let content_sha256 = canonical_sha256(&extracted_json, "capability probe evidence")?;
    Ok(DiscoveryEvidenceRecord {
        id,
        session_id: snapshot.session.id.clone(),
        kind: DiscoveryEvidenceKind::JsonDocument,
        source_url: source_url.clone(),
        content_sha256,
        extracted_json,
        fetched_at: observation.observed_at,
    })
}

const fn probe_slug(probe: CapabilityProbeKind) -> &'static str {
    match probe {
        CapabilityProbeKind::Streaming => "streaming",
        CapabilityProbeKind::Reasoning => "reasoning",
        CapabilityProbeKind::StructuredOutput => "structured-output",
        CapabilityProbeKind::ToolCalling => "tool-calling",
        CapabilityProbeKind::PromptCaching => "prompt-caching",
    }
}

fn list_models_for_draft(
    runtime: &Handle,
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    credential: Option<&str>,
    cancelled: watch::Receiver<bool>,
) -> CoreResult<()> {
    if snapshot.session.state != DiscoveryState::ListingModels {
        return Err(CoreError::invalid(
            "model listing state changed unexpectedly",
        ));
    }
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("model listing has no template"))?;
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("model listing has no connection"))?;
    let listing = AdapterRegistry::new().build_model_listing(template, connection)?;
    let listed =
        runtime.block_on(listing.list_models(ModelListRequest::new(credential, cancelled)))?;
    ensure_listing_does_not_reflect_credential(&listed, credential)?;
    apply_listed_models_to_draft(draft, &listed.models, Utc::now())
}

fn apply_listed_models_to_draft(
    draft: &mut DiscoveryWorkingDraft,
    listed_models: &[lorepia_providers::ListedModel],
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("model listing has no template"))?
        .clone();
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("model listing has no connection"))?
        .clone();
    let (routes, _, _) = reconcile_input_routes(
        &connection.id,
        template.api_family,
        &[],
        listed_models,
        observed_at,
    )?;
    // `reconcile_input_routes` retains only the same closed, bounded,
    // credential-scanned provider metadata accepted by durable model sync.
    // Persisting that normalized projection lets the first reviewed discovery
    // graph enforce model-specific parameter controls immediately; no raw
    // provider response bytes enter the review or storage graph.
    let observations = provider_api_capability_observations(&routes, listed_models, observed_at)?;
    let presets = if template_accepts_empty_preset(&template)? {
        routes
            .iter()
            .map(|route| initial_generation_preset(&route.id, &template, observed_at))
            .collect()
    } else {
        Vec::new()
    };
    let mut connected = connection.clone();
    connected.status = ConnectionStatus::Connected;
    connected.updated_at = observed_at;
    draft.connection = Some(connected);
    draft.routes = routes;
    draft.observations = observations;
    draft.presets = presets;
    Ok(())
}

fn ensure_listing_does_not_reflect_credential(
    listed: &lorepia_providers::ModelListResult,
    credential: Option<&str>,
) -> CoreResult<()> {
    let Some(secret) = credential.filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    if listed.models.iter().any(|model| {
        model.model_id.contains(secret)
            || model
                .display_name
                .as_deref()
                .is_some_and(|value| value.contains(secret))
            || model
                .supported_generation_methods
                .iter()
                .any(|value| value.contains(secret))
            || serde_json::to_string(&model.capabilities).is_ok_and(|value| value.contains(secret))
    }) {
        return Err(CoreError::new(
            CoreErrorCode::ProviderUnavailable,
            "provider model response reflected credential material",
            false,
        ));
    }
    Ok(())
}

fn model_candidates(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<Vec<StoredDiscoveryCandidate>> {
    draft
        .routes
        .iter()
        .map(|route| {
            Ok(StoredDiscoveryCandidate {
                candidate: DiscoveryCandidate {
                    id: DiscoveryCandidateId::parse(deterministic_id(
                        &snapshot.session.id,
                        0,
                        &format!("model-route:{}", route.id.as_str()),
                    ))
                    .map_err(|error| {
                        CoreError::internal(format!("candidate id failed: {error}"))
                    })?,
                    session_id: snapshot.session.id.clone(),
                    summary: DiscoveryCandidateSummary::ModelRoute {
                        model_id: route.model_id.clone(),
                    },
                    evidence_ids: Vec::new(),
                    created_at: snapshot.created_at,
                },
                proposed_revision: snapshot.session.revision,
            })
        })
        .collect()
}

impl crate::app::Core {
    fn prepared_discovery_credential_reservation_id(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
    ) -> CoreResult<Option<String>> {
        let snapshot = self.provider_discovery().get(session_id)?;
        if snapshot.session.revision != expected_revision
            || snapshot.session.state != DiscoveryState::Committing
            || snapshot.session.input.credential_ref.is_none()
        {
            return Ok(None);
        }
        let context = self
            .provider_discovery()
            .credential_install_recovery_context(session_id)?;
        Ok(
            (context.operation_status == DiscoveryOperationStatus::Prepared
                && context.native_execution_id.is_none())
            .then_some(context.native_execution_reservation_id)
            .flatten(),
        )
    }

    pub fn begin_provider_discovery_known(
        &self,
        input: SanitizedDiscoveryInput,
        template_id: ProviderTemplateId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.begin_provider_discovery_known_with_credential_authority(input, template_id, None)
    }

    pub fn begin_provider_discovery_known_with_credential_authority(
        &self,
        input: SanitizedDiscoveryInput,
        template_id: ProviderTemplateId,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().begin_with_credential_authority(
            input,
            ProviderDiscoverySource::known_provider_id(template_id),
            credential_authority,
        )
    }

    /// Returns every session with an unfinished durable operation for backend
    /// startup recovery. Unlike the user-facing history query, this scan is
    /// complete and is not capped to the latest sessions.
    pub fn list_unfinished_provider_discovery_recovery_candidates(
        &self,
    ) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        self.provider_discovery().unfinished_recovery_candidates()
    }

    /// Returns every credential-bound commit that requires native vault
    /// reconciliation before generic startup recovery may classify its WAL.
    pub fn list_provider_discovery_credential_recovery_candidates(
        &self,
    ) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        self.provider_discovery().credential_recovery_candidates()
    }

    pub fn list_provider_discovery_approvals(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Vec<DiscoveryApprovalRecord>> {
        self.provider_discovery().approvals(session_id)
    }

    pub fn get_provider_discovery_approval_proposal(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<ProviderDiscoveryApprovalProposal>> {
        self.provider_discovery().approval_proposal(session_id)
    }

    pub fn get_provider_discovery_review_proposal(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<ProviderDiscoveryReviewProposal>> {
        self.provider_discovery().review_proposal(session_id)
    }

    pub fn get_provider_discovery_assistant_resume_boundary(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<ProviderDiscoveryAssistantResumeBoundary>> {
        self.provider_discovery()
            .assistant_resume_boundary(session_id)
    }

    pub fn commit_provider_discovery(
        &self,
        session_id: &DiscoverySessionId,
        credential_confirmation: Option<&ProviderDiscoveryCredentialCommitConfirmation>,
    ) -> CoreResult<ProviderConnection> {
        self.provider_discovery()
            .commit(session_id, credential_confirmation)
    }

    pub fn get_provider_discovery_credential_install_context(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        self.provider_discovery()
            .credential_install_context(session_id)
    }

    pub fn get_provider_discovery_credential_lease_context(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialLeaseContext> {
        self.provider_discovery()
            .credential_lease_context(session_id)
    }

    pub fn get_provider_discovery_credential_install_recovery_context(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        self.provider_discovery()
            .credential_install_recovery_context(session_id)
    }

    pub fn get_provider_discovery_credential_compensation_authority(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialAuthority> {
        self.provider_discovery()
            .credential_compensation_authority(session_id)
    }

    pub fn reserve_provider_discovery_credential_install(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        let reserved = self.provider_discovery().reserve_credential_install(
            session_id,
            expected_revision,
            expected_operation_id,
            expected_attempt_id,
            expected_plan_sha256,
        )?;
        let physical_authority_id = reserved
            .native_execution_reservation_id
            .as_deref()
            .ok_or_else(|| CoreError::internal("credential reservation has no physical id"))?;
        self.remember_discovery_credential_reservation(physical_authority_id)?;
        Ok(reserved)
    }

    pub fn start_provider_discovery_credential_install(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
        expected_native_execution_reservation_id: &str,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        let preflight = self
            .provider_discovery()
            .credential_install_context(session_id)?;
        if preflight.session_revision != expected_revision
            || &preflight.operation_id != expected_operation_id
            || &preflight.commit_attempt_id != expected_attempt_id
            || preflight.commit_plan_sha256 != expected_plan_sha256
            || preflight.commit_phase != DiscoveryCommitPhase::Prepared
            || preflight.operation_status != DiscoveryOperationStatus::Prepared
            || preflight.native_execution_id.is_some()
            || preflight.native_execution_reservation_id.as_deref()
                != Some(expected_native_execution_reservation_id)
        {
            return Err(CoreError::invalid(
                "credential installation reservation changed before process-local start",
            ));
        }
        self.consume_discovery_credential_reservation(expected_native_execution_reservation_id)?;
        self.provider_discovery().start_credential_install(
            session_id,
            expected_revision,
            expected_operation_id,
            expected_attempt_id,
            expected_plan_sha256,
            expected_native_execution_reservation_id,
        )
    }

    pub fn attest_provider_discovery_credential_install_no_effect(
        &self,
        session_id: &DiscoverySessionId,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
        expected_native_execution_id: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .attest_credential_install_no_effect(
                session_id,
                expected_operation_id,
                expected_attempt_id,
                expected_plan_sha256,
                expected_native_execution_id,
            )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn mark_provider_discovery_credential_install_durability_unknown(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
        expected_native_execution_id: &str,
        expected_connection_id: &ProviderConnectionId,
        expected_connection_binding_sha256: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .mark_credential_install_durability_unknown(
                session_id,
                expected_revision,
                expected_operation_id,
                expected_attempt_id,
                expected_plan_sha256,
                expected_native_execution_id,
                expected_connection_id,
                expected_connection_binding_sha256,
            )
    }

    pub fn recover_provider_discovery(
        &self,
        recovered_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryRecoveryResult>> {
        for snapshot in self.provider_discovery().credential_recovery_candidates()? {
            let context = self
                .provider_discovery()
                .credential_install_recovery_context(&snapshot.session.id)?;
            if context.operation_status == DiscoveryOperationStatus::Prepared
                && context.native_execution_id.is_none()
                && let Some(physical_authority_id) =
                    context.native_execution_reservation_id.as_deref()
            {
                // Consume the process-local capability before recovery. If the
                // durable transition later fails, fail closed: this physical
                // reservation still cannot be retried or adopted.
                self.forget_discovery_credential_reservation(physical_authority_id)?;
            }
        }
        self.provider_discovery().recover_startup(recovered_at)
    }

    pub fn list_provider_discovery_compensation_steps(
        &self,
        attempt_id: &DiscoveryCommitAttemptId,
    ) -> CoreResult<Vec<lorepia_storage::DiscoveryCompensationRecord>> {
        self.provider_discovery().compensation_steps(attempt_id)
    }

    pub fn continue_provider_discovery_compensation(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().continue_compensation(session_id)
    }

    pub fn start_provider_discovery_credential_compensation(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<lorepia_storage::DiscoveryCompensationRecord> {
        self.provider_discovery()
            .start_credential_compensation(session_id, step_id)
    }

    pub fn complete_provider_discovery_credential_compensation(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .complete_credential_compensation(session_id, step_id)
    }

    pub fn fail_provider_discovery_credential_compensation(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
        failure: DiscoveryFailure,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .fail_credential_compensation(session_id, step_id, failure)
    }

    pub fn mark_provider_discovery_credential_compensation_unknown(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .mark_credential_compensation_unknown(session_id, step_id)
    }

    pub fn resume_provider_discovery_compensation(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().resume_compensation(session_id)
    }

    pub fn run_provider_discovery_assistant_turn(
        &self,
        session_id: &DiscoverySessionId,
        estimate: AssistantCallEstimate,
        credential: Option<&str>,
    ) -> CoreResult<AssistantHostAction> {
        let snapshot = self.provider_discovery().get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let assistant_route_id = draft
            .assistant
            .as_ref()
            .ok_or_else(|| CoreError::internal("setup assistant snapshot is missing"))?
            .assistant_route_id()
            .clone();
        let settings = self.get_settings()?;
        let selected_route_id = settings.selected_model_route_id.ok_or_else(|| {
            CoreError::invalid("setup assistant requires a selected model route and preset")
        })?;
        let selected_preset_id = settings.selected_generation_preset_id.ok_or_else(|| {
            CoreError::invalid("setup assistant requires a selected model route and preset")
        })?;
        if selected_route_id != assistant_route_id {
            return Err(CoreError::invalid(
                "setup assistant route must match the selected model route",
            ));
        }
        let target = GenerationTarget {
            model_route_id: selected_route_id.clone(),
            generation_preset_id: selected_preset_id,
        };
        let resolved = crate::app::resolve_generation_target(self, &target)?;
        let route = self.storage().get_model_route(&selected_route_id)?;
        if resolved.model != route.model_id {
            return Err(CoreError::internal(
                "selected setup assistant target resolved to a different model",
            ));
        }
        self.provider_discovery().run_assistant_with_provider(
            session_id,
            &route,
            resolved.provider,
            estimate,
            credential,
        )
    }

    pub fn approve_provider_discovery_assistant_retry(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .approve_assistant_retry(session_id)
    }

    pub fn resume_provider_discovery_assistant_core_host_action(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .resume_assistant_core_host_action(session_id)
    }

    pub fn request_provider_discovery_assistant_revision(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .request_assistant_draft_revision(session_id)
    }

    pub fn accept_provider_discovery_assistant_draft(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().accept_assistant_draft(session_id)
    }

    pub fn record_provider_discovery_assistant_failure(
        &self,
        session_id: &DiscoverySessionId,
        kind: AssistantFailureKind,
        retryable: bool,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .record_assistant_failure(session_id, kind, retryable)
    }

    pub fn interrupt_provider_discovery_assistant(
        &self,
        session_id: &DiscoverySessionId,
        outcome: DiscoveryInterruptionOutcome,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .interrupt_assistant(session_id, outcome)
    }

    pub fn restart_provider_discovery_assistant_after_interruption(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .restart_assistant_after_interruption(session_id)
    }
}

fn deterministic_error(
    error: crate::provider_discovery_deterministic::DeterministicDiscoveryError,
) -> CoreError {
    let (code, message) = match error.kind() {
        DeterministicDiscoveryErrorKind::InvalidSource
        | DeterministicDiscoveryErrorKind::InvalidDocumentUrl
        | DeterministicDiscoveryErrorKind::InvalidFetchBudget
        | DeterministicDiscoveryErrorKind::CurlParseRejected => (
            CoreErrorCode::InvalidInput,
            "provider discovery source was rejected",
        ),
        DeterministicDiscoveryErrorKind::KnownProviderNotFound => {
            (CoreErrorCode::NotFound, "known provider was not found")
        }
        DeterministicDiscoveryErrorKind::ProviderContractUnavailable
        | DeterministicDiscoveryErrorKind::EvidenceSerializationFailed
        | DeterministicDiscoveryErrorKind::UnsafeEvidence => (
            CoreErrorCode::UnsupportedContent,
            "provider discovery evidence could not be used",
        ),
    };
    CoreError::new(code, message, false)
}

fn cancelled_commit_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::Cancelled,
        "provider discovery commit was cancelled before graph publication",
        false,
    )
}

fn active_discovery_templates(storage: &Storage) -> CoreResult<Vec<ProviderTemplate>> {
    let mut active = std::collections::BTreeMap::<ProviderTemplateId, ProviderTemplate>::new();
    for template in storage.list_provider_templates()? {
        if template.source != TemplateSource::SignedCatalog {
            insert_active_discovery_template(&mut active, template)?;
        }
    }

    let projection = operational_provider_catalog_projection_for_storage(storage, Utc::now())?;
    for template in projection.provider_templates() {
        insert_active_discovery_template(&mut active, template)?;
    }
    Ok(active.into_values().collect())
}

fn current_discovery_catalog_authority(
    storage: &Storage,
    template: &ProviderTemplate,
    now: DateTime<Utc>,
) -> CoreResult<Option<DiscoveryCatalogAuthorityBinding>> {
    if template.source != TemplateSource::SignedCatalog {
        return Ok(None);
    }
    operational_provider_catalog_projection_for_storage(storage, now)?
        .discovery_authority_binding(template, now)
}

fn revalidate_discovery_catalog_authority(
    storage: &Storage,
    draft: &DiscoveryWorkingDraft,
    now: DateTime<Utc>,
) -> CoreResult<()> {
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("provider discovery has no template authority"))?;
    if template.source != TemplateSource::SignedCatalog {
        return if draft.catalog_authority.is_none() {
            Ok(())
        } else {
            Err(CoreError::invalid(
                "non-catalog provider discovery carries signed catalog authority",
            ))
        };
    }
    let current = current_discovery_catalog_authority(storage, template, now)?;
    if current != draft.catalog_authority {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "signed catalog authority changed or expired; restart provider discovery",
            true,
        ));
    }
    Ok(())
}

fn revalidate_prepared_discovery_catalog_authority(
    storage: &Storage,
    draft: &DiscoveryWorkingDraft,
    phase: DiscoveryCommitPhase,
) -> CoreResult<()> {
    if phase == DiscoveryCommitPhase::Prepared {
        revalidate_discovery_catalog_authority(storage, draft, Utc::now())?;
    }
    Ok(())
}

fn insert_active_discovery_template(
    active: &mut std::collections::BTreeMap<ProviderTemplateId, ProviderTemplate>,
    candidate: ProviderTemplate,
) -> CoreResult<()> {
    match active.get(&candidate.id) {
        Some(existing) if existing.manifest_version > candidate.manifest_version => Ok(()),
        Some(existing)
            if existing.manifest_version == candidate.manifest_version
                && existing != &candidate =>
        {
            Err(CoreError::internal(
                "active provider catalog contains conflicting immutable template versions",
            ))
        }
        _ => {
            active.insert(candidate.id.clone(), candidate);
            Ok(())
        }
    }
}

fn require_active_discovery_network_authority(
    options: &ProviderDiscoveryConnectionOptions,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    options
        .require_active_local_network_approval_at(observed_at)
        .map_err(|error| {
            CoreError::new(
                CoreErrorCode::InvalidInput,
                format!("provider discovery network authority is inactive: {error}"),
                true,
            )
        })
}

fn require_active_discovery_commit_authority(
    snapshot: &DiscoverySessionSnapshot,
) -> CoreResult<()> {
    if snapshot.session.state != DiscoveryState::Committing {
        return Err(CoreError::invalid(
            "provider discovery is not awaiting an atomic commit",
        ));
    }
    require_active_discovery_network_authority(
        &snapshot.session.input.connection_options,
        Utc::now(),
    )
}

/// Bounded project-owned fixtures for downstream Shell adapter tests.
#[cfg(feature = "test-support")]
#[path = "provider_discovery/test_support.rs"]
pub mod test_support;

#[cfg(test)]
#[path = "provider_discovery/tests/mod.rs"]
mod policy_tests;
