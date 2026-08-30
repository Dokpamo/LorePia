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

mod actions;
mod assistant;
mod assistant_lifecycle;
mod assistant_runtime;
mod begin;
mod deterministic;
mod driver;
mod known_provider;
mod probes;
mod types;
mod views;

pub use actions::provider_discovery_action_envelope;
pub(crate) use assistant::resumable_assistant_operation_ids;
pub use assistant::{
    ProviderDiscoveryAssistantResumeAction, ProviderDiscoveryAssistantResumeBoundary,
};
pub(crate) use types::ProviderDiscoveryOrchestrator;

#[cfg(test)]
use actions::write_canonical_json;
use actions::{canonical_sha256, sha256_hex};
use assistant::{
    assistant_checkpoint, assistant_error, assistant_proposal, assistant_structured_output_error,
    cancel_assistant_snapshot, corrupted_assistant_resume_boundary, grant_assistant_snapshot,
    initialize_assistant, record_deterministic_assistant_claims, redacted_assistant_evidence,
    restored_assistant, synchronize_assistant_snapshot,
};
#[cfg(test)]
use assistant::{decoder_slug, endpoint_claim};
#[cfg(test)]
use assistant_runtime::{api_family_slug, run_setup_assistant_provider_call};
use begin::{
    additional_curl_url_policy, additional_document_url_policy,
    credential_bearing_curl_requires_handoff, discovery_url_policy,
};
use deterministic::{
    deterministic_artifacts, deterministic_error, install_graph_seed,
    install_graph_seed_with_embedded_base, revalidate_discovery_catalog_authority,
    revalidate_prepared_discovery_catalog_authority, select_candidate,
};
use driver::{
    EffectCompletion, hydrate_working_draft, operation_for_effect, transition_error,
    working_draft_value,
};
use probes::standard_probe_budget;
#[cfg(test)]
use probes::{ProbeExecution, approved_probe_routes, probe_draft};
#[cfg(any(test, feature = "test-support"))]
use probes::{apply_listed_models_to_draft, model_candidates};
use types::{DiscoverySourceIntent, DiscoveryWorkingDraft};
pub use types::{
    ProviderCurlInspection, ProviderDiscoveryAdditionalEvidence, ProviderDiscoveryCurlInput,
    ProviderDiscoverySource,
};

impl ProviderDiscoveryOrchestrator<'_> {
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
}

fn cancelled_commit_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::Cancelled,
        "provider discovery commit was cancelled before graph publication",
        false,
    )
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
