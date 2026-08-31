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

mod actions;
mod approval;
mod assistant;
mod assistant_lifecycle;
mod assistant_runtime;
mod begin;
mod credential;
mod credential_install;
mod deterministic;
mod driver;
mod known_provider;
mod probes;
mod types;
mod views;

pub use actions::provider_discovery_action_envelope;
pub use approval::{ProviderDiscoveryApprovalProposal, ProviderDiscoveryReviewProposal};
pub(crate) use assistant::resumable_assistant_operation_ids;
pub use assistant::{
    ProviderDiscoveryAssistantResumeAction, ProviderDiscoveryAssistantResumeBoundary,
};
pub use credential::{
    ProviderDiscoveryCredentialAuthority, ProviderDiscoveryCredentialCommitConfirmation,
    ProviderDiscoveryCredentialInstallContext, ProviderDiscoveryCredentialLeaseContext,
};
pub(crate) use types::ProviderDiscoveryOrchestrator;

#[cfg(test)]
use actions::write_canonical_json;
use actions::{canonical_sha256, sha256_hex};
use approval::{
    approval_proposal_for, approval_record, approved_probe_budget, build_review,
    credential_origin_grant, credential_origin_proposal, probe_proposal, require_approval_binding,
    require_approval_id, sanitized_graph_sha256,
};
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
use credential::apply_credential_origin_scope;
#[cfg(test)]
use credential::{
    native_credential_execution_context_ids, validate_credential_origin_approval,
    validated_discovery_credential_binding_sha256,
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

fn origin_from_http_url(url: &HttpUrl) -> CoreResult<CanonicalOrigin> {
    let parsed = url::Url::parse(url.as_str())
        .map_err(|_| CoreError::invalid("provider discovery URL is invalid"))?;
    let origin = parsed.origin().ascii_serialization();
    CanonicalOrigin::parse(&origin)
        .map_err(|error| CoreError::invalid(format!("provider origin is invalid: {error}")))
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

    pub fn commit_provider_discovery(
        &self,
        session_id: &DiscoverySessionId,
        credential_confirmation: Option<&ProviderDiscoveryCredentialCommitConfirmation>,
    ) -> CoreResult<ProviderConnection> {
        self.provider_discovery()
            .commit(session_id, credential_confirmation)
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
