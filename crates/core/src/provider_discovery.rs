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
    DiscoveredProviderGraph, DiscoveryCommitPhase, DiscoveryCompletedOperationWrite,
    DiscoveryEvidenceKind, DiscoveryEvidenceRecord, DiscoveryJsonUpdate,
    DiscoveryNativeCredentialExecutionRecord, DiscoveryNativeCredentialExecutionReservation,
    DiscoveryNativeCredentialStoreAttemptStart, DiscoveryNativeNoEffectAttestationWrite,
    DiscoveryOperationStatus, DiscoveryOutboxEvent, DiscoveryRecoveryResult,
    DiscoverySessionSnapshot, DiscoveryTransitionWrite, DurableOperationOutcome,
    PreparedDiscoveryCommit, PreparedDiscoveryCompensationStep, ProviderCredentialAccessAuthority,
    Storage, StoredDiscoveryCandidate,
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

/// Secret-free options for a cURL-only discovery start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCurlInput {
    pub connection_id: ProviderConnectionId,
    pub display_name: String,
    pub docs_url: Option<HttpUrl>,
    pub credential_ref: Option<CredentialRef>,
    pub preferred_assistant: Option<ModelRouteId>,
    pub connection_options: ProviderDiscoveryConnectionOptions,
    pub supplied_evidence_ids: Vec<EvidenceId>,
}

/// One-shot cURL inspection result.
///
/// This type is intentionally not serializable. Its manual `Debug` never
/// exposes the extracted credential. Callers should immediately move that
/// credential to the native vault, retain only the opaque credential
/// reference, and pass `redacted_curl()` to discovery.
pub struct ProviderCurlInspection {
    site_url: HttpUrl,
    origin: CanonicalOrigin,
    redacted_curl: String,
    auth_hints: Vec<CurlAuthHint>,
    evidence: ParsedCurlEvidence,
    extracted_credential: Option<SecretBytes>,
}

impl std::fmt::Debug for ProviderCurlInspection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCurlInspection")
            .field("site_url", &self.site_url)
            .field("origin", &self.origin)
            .field("redacted_curl", &self.redacted_curl)
            .field("auth_hints", &self.auth_hints)
            .field(
                "extracted_credential_present",
                &self.extracted_credential.is_some(),
            )
            .finish_non_exhaustive()
    }
}

impl ProviderCurlInspection {
    pub fn site_url(&self) -> &HttpUrl {
        &self.site_url
    }

    pub fn origin(&self) -> &CanonicalOrigin {
        &self.origin
    }

    pub fn redacted_curl(&self) -> &str {
        &self.redacted_curl
    }

    pub fn auth_hints(&self) -> &[CurlAuthHint] {
        &self.auth_hints
    }

    pub fn evidence(&self) -> &ParsedCurlEvidence {
        &self.evidence
    }

    pub fn extracted_credential(&self) -> Option<&[u8]> {
        self.extracted_credential
            .as_ref()
            .map(SecretBytes::expose_to_vault)
    }

    pub fn into_parts(self) -> (ParsedCurlEvidence, Option<SecretBytes>) {
        (self.evidence, self.extracted_credential)
    }
}

/// A source selector with no serializable raw cURL representation.
///
/// Site and known-provider sources are reconstructed from the sanitized input.
/// A cURL source is one-shot: if the process stops before it is reduced to a
/// safe deterministic result, the user must explicitly restart with a newly
/// supplied source.
pub struct ProviderDiscoverySource {
    intent: DiscoverySourceIntent,
    transient: Option<DeterministicDiscoverySource>,
    declared_connection_options: Option<ProviderDiscoveryConnectionOptions>,
    derived_site_url: Option<HttpUrl>,
}

/// One fresh evidence source accepted only while discovery is waiting for more
/// evidence.
///
/// The document variant is already secret-free. The cURL variant owns a
/// one-shot, zeroizing input and therefore implements neither serialization,
/// cloning, nor debug formatting.
pub enum ProviderDiscoveryAdditionalEvidence {
    DocumentUrl(HttpUrl),
    Curl(SecretCurlInput),
}

impl ProviderDiscoveryAdditionalEvidence {
    pub const fn document_url(url: HttpUrl) -> Self {
        Self::DocumentUrl(url)
    }

    pub const fn curl(input: SecretCurlInput) -> Self {
        Self::Curl(input)
    }
}

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

    pub fn site() -> Self {
        Self {
            intent: DiscoverySourceIntent::Site,
            transient: None,
            declared_connection_options: None,
            derived_site_url: None,
        }
    }

    pub fn curl(
        input: SecretCurlInput,
        connection_options: ProviderDiscoveryConnectionOptions,
    ) -> CoreResult<Self> {
        let policy = unissued_discovery_url_policy(&connection_options)?;
        let inspection = inspect_curl(input)
            .map_err(|_| CoreError::invalid("pasted cURL input was rejected"))?;
        let (evidence, extracted_credential) = inspection.into_parts();
        if extracted_credential.is_some() {
            drop(extracted_credential);
            return Err(credential_bearing_curl_requires_handoff());
        }
        Self::sanitized_curl(evidence, policy, connection_options)
    }

    fn sanitized_curl(
        evidence: ParsedCurlEvidence,
        policy: UrlPolicy,
        connection_options: ProviderDiscoveryConnectionOptions,
    ) -> CoreResult<Self> {
        let derived_site_url = HttpUrl::parse(evidence.origin.as_str())
            .map_err(|error| CoreError::invalid(format!("invalid cURL origin: {error}")))?;
        let transient = DeterministicDiscoverySource::sanitized_curl_with_policy(evidence, policy)
            .map_err(deterministic_error)?;
        Ok(Self {
            intent: DiscoverySourceIntent::Curl,
            transient: Some(transient),
            declared_connection_options: Some(connection_options),
            derived_site_url: Some(derived_site_url),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DiscoverySourceIntent {
    KnownProvider {
        template_id: lorepia_domain::ProviderTemplateId,
    },
    Site,
    Curl,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryWorkingDraft {
    schema_version: u32,
    source: DiscoverySourceIntent,
    deterministic: Option<DeterministicDiscoveryOutput>,
    evidence_ids: Vec<EvidenceId>,
    extra_evidence_ids: Vec<EvidenceId>,
    selected_candidate_id: Option<DiscoveryCandidateId>,
    template: Option<ProviderTemplate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    catalog_authority: Option<DiscoveryCatalogAuthorityBinding>,
    connection: Option<ProviderConnection>,
    routes: Vec<ModelRoute>,
    observations: Vec<CapabilityObservation>,
    presets: Vec<GenerationPreset>,
    credential_approval_id: Option<DiscoveryApprovalId>,
    probe_route_ids: Vec<ModelRouteId>,
    probe_failure_count: u32,
    assistant: Option<AssistantEngineSnapshot>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    assistant_evidence_claims: BTreeMap<EvidenceId, Vec<EvidenceClaim>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    assistant_approval_binding: Option<DiscoveryApprovalBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    assistant_more_evidence_questions: Vec<UnresolvedQuestion>,
}

impl DiscoveryWorkingDraft {
    fn new(source: DiscoverySourceIntent) -> Self {
        Self {
            schema_version: WORKING_DRAFT_SCHEMA_VERSION,
            source,
            deterministic: None,
            evidence_ids: Vec::new(),
            extra_evidence_ids: Vec::new(),
            selected_candidate_id: None,
            template: None,
            catalog_authority: None,
            connection: None,
            routes: Vec::new(),
            observations: Vec::new(),
            presets: Vec::new(),
            credential_approval_id: None,
            probe_route_ids: Vec::new(),
            probe_failure_count: 0,
            assistant: None,
            assistant_evidence_claims: BTreeMap::new(),
            assistant_approval_binding: None,
            assistant_more_evidence_questions: Vec::new(),
        }
    }
}

/// Coordinates one discovery graph against a Storage and Core runtime.
pub(crate) struct ProviderDiscoveryOrchestrator<'a> {
    storage: &'a Storage,
    runtime: &'a Handle,
    recovery_owner: DiscoveryRecoveryOwner,
}

impl<'a> ProviderDiscoveryOrchestrator<'a> {
    pub const fn new(
        storage: &'a Storage,
        runtime: &'a Handle,
        recovery_owner: DiscoveryRecoveryOwner,
    ) -> Self {
        Self {
            storage,
            runtime,
            recovery_owner,
        }
    }

    pub fn get(&self, session_id: &DiscoverySessionId) -> CoreResult<DiscoverySessionSnapshot> {
        self.storage.get_discovery_session(session_id)
    }

    pub fn candidates(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Vec<StoredDiscoveryCandidate>> {
        self.storage
            .list_discovery_candidates(session_id, MAX_DISCOVERY_ROWS)
    }

    pub fn evidence(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Vec<DiscoveryEvidenceRecord>> {
        self.storage
            .list_discovery_evidence(session_id, MAX_DISCOVERY_ROWS)
    }

    pub fn review(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<DiscoveryReviewDiff>> {
        self.storage.get_discovery_review(session_id)
    }

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

    pub fn poll_outbox(
        &self,
        limit: u32,
        available_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
        self.storage.poll_discovery_events(limit, available_at)
    }

    pub fn poll_outbox_for_session(
        &self,
        session_id: &DiscoverySessionId,
        limit: u32,
        available_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
        self.storage
            .poll_discovery_events_for_session(session_id, limit, available_at)
    }

    pub fn ack_outbox(
        &self,
        event_id: &DiscoveryEventId,
        delivered_at: DateTime<Utc>,
    ) -> CoreResult<bool> {
        self.storage.ack_discovery_event(event_id, delivered_at)
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

    pub fn approvals(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Vec<DiscoveryApprovalRecord>> {
        self.storage
            .list_discovery_approvals(session_id, MAX_DISCOVERY_ROWS)
    }

    pub fn list(&self, limit: u32) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        self.storage.list_discovery_sessions(limit)
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

    #[allow(clippy::unused_self)]
    pub fn inspect_curl(
        &self,
        input: SecretCurlInput,
        connection_options: &ProviderDiscoveryConnectionOptions,
    ) -> CoreResult<ProviderCurlInspection> {
        let policy = unissued_discovery_url_policy(connection_options)?;
        let inspection = inspect_curl(input)
            .map_err(|_| CoreError::invalid("pasted cURL input was rejected"))?;
        let (evidence, extracted_credential) = inspection.into_parts();
        DeterministicDiscoverySource::sanitized_curl_with_policy(evidence.clone(), policy)
            .map_err(deterministic_error)?;
        let site_url = HttpUrl::parse(evidence.origin.as_str())
            .map_err(|error| CoreError::invalid(format!("invalid cURL origin: {error}")))?;
        Ok(ProviderCurlInspection {
            site_url,
            origin: evidence.origin.clone(),
            redacted_curl: evidence.redacted_curl.clone(),
            auth_hints: evidence.auth_hints.clone(),
            evidence,
            extracted_credential,
        })
    }

    /// Starts discovery directly from a cURL command. The cURL origin becomes
    /// the sanitized site URL, so no separate site/docs URL is required.
    ///
    /// If the command contains a credential, callers must first use
    /// [`Self::inspect_curl`], move the returned secret into the native vault,
    /// and call this method with the inspection's redacted cURL plus the opaque
    /// credential reference.
    pub fn begin_curl_with_credential_authority(
        &self,
        input: ProviderDiscoveryCurlInput,
        curl: SecretCurlInput,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let source = ProviderDiscoverySource::curl(curl, input.connection_options.clone())?;
        let site_url = source
            .derived_site_url
            .clone()
            .ok_or_else(|| CoreError::internal("sanitized cURL lost its derived origin"))?;
        self.begin_with_credential_authority(
            SanitizedDiscoveryInput {
                connection_id: input.connection_id,
                display_name: input.display_name,
                site_url,
                docs_url: input.docs_url,
                credential_ref: input.credential_ref,
                preferred_assistant: input.preferred_assistant,
                connection_options: input.connection_options,
                supplied_evidence_ids: input.supplied_evidence_ids,
            },
            source,
            credential_authority,
        )
    }

    /// Starts a durable discovery and immediately executes only its prepared
    /// non-persistent effects. A raw cURL value is consumed and reduced to a
    /// secret-free deterministic result before any draft is serialized.
    pub fn begin_with_credential_authority(
        &self,
        mut input: SanitizedDiscoveryInput,
        mut source: ProviderDiscoverySource,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let occurred_at = Utc::now();
        input
            .connection_options
            .issue_local_network_approval_at(occurred_at)
            .map_err(|error| CoreError::invalid(format!("invalid discovery input: {error}")))?;
        if let Some(declared) = source.declared_connection_options.as_mut() {
            declared
                .issue_local_network_approval_at(occurred_at)
                .map_err(|error| {
                    CoreError::invalid(format!("invalid cURL connection options: {error}"))
                })?;
        }
        input
            .validate()
            .map_err(|error| CoreError::invalid(format!("invalid discovery input: {error}")))?;
        if input.connection_options.network_mode == ProviderNetworkMode::ApprovedLocalNetwork
            && matches!(source.intent, DiscoverySourceIntent::Site)
        {
            return Err(approved_lan_web_discovery_disabled());
        }
        if input
            .credential_ref
            .as_ref()
            .is_some_and(|reference| reference.as_str() != input.connection_id.as_str())
        {
            return Err(CoreError::invalid(
                "discovery credential reference must equal the intended connection identifier",
            ));
        }
        if source
            .declared_connection_options
            .as_ref()
            .is_some_and(|declared| declared != &input.connection_options)
        {
            return Err(CoreError::invalid(
                "cURL connection options do not match the sanitized discovery input",
            ));
        }
        let mut draft = DiscoveryWorkingDraft::new(source.intent.clone());
        if let Some(transient) = source.transient.take() {
            draft.deterministic = Some(
                self.runtime
                    .block_on(DeterministicDiscoveryExecutor::new().execute(transient))
                    .map_err(deterministic_error)?,
            );
        }
        let session_id = DiscoverySessionId::from(Uuid::new_v4().to_string());
        let initial = ProviderDiscoverySession::new(session_id.clone(), input)
            .map_err(|error| CoreError::invalid(format!("invalid discovery input: {error}")))?;
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            0,
            ProviderDiscoveryAction::Begin,
        )?;
        let transition = initial.apply(&envelope).map_err(transition_error)?;
        let write = DiscoveryTransitionWrite {
            transition,
            draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
            review: DiscoveryJsonUpdate::Clear,
            new_evidence: Vec::new(),
            new_candidates: Vec::new(),
            approval: None,
            new_operation_id: Some(DiscoveryOperationId::new()),
            completed_operation: None,
            prepared_commit: None,
            provider_graph: None,
            occurred_at,
        };
        self.storage
            .begin_discovery_session_with_credential_authority(
                &initial,
                &write,
                credential_authority.as_ref(),
            )?;
        let (_cancel, cancelled) = watch::channel(false);
        self.drive_nonpersistent(&session_id, None, cancelled)
    }

    /// Applies one user action with revision/idempotency and exact approval
    /// binding, then executes any resulting non-persistent effect.
    pub fn continue_discovery(
        &self,
        session_id: &DiscoverySessionId,
        envelope: DiscoveryActionEnvelope,
        credential: Option<&str>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let (_cancel, cancelled) = watch::channel(false);
        self.continue_discovery_with_cancellation(session_id, envelope, credential, cancelled)
    }

    pub fn continue_discovery_with_cancellation(
        &self,
        session_id: &DiscoverySessionId,
        envelope: DiscoveryActionEnvelope,
        credential: Option<&str>,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        Self::validate_envelope(&envelope)?;
        if self
            .storage
            .find_discovery_action_replay(
                session_id,
                &envelope.id,
                &envelope.request_sha256,
                envelope.action.kind(),
            )?
            .is_some()
        {
            return self.get(session_id);
        }
        if !is_public_discovery_action(&envelope.action) {
            return Err(CoreError::invalid(
                "internal discovery completion actions are not accepted at the public boundary",
            ));
        }
        let snapshot = self.get(session_id)?;
        if snapshot.session.id != *session_id {
            return Err(CoreError::invalid("discovery session identifier mismatch"));
        }
        let mut draft = hydrate_working_draft(&snapshot)?;
        let is_cancel = matches!(&envelope.action, ProviderDiscoveryAction::Cancel);
        let occurred_at = Utc::now();
        let (approval, review_update, prepared_commit) =
            self.prepare_user_action(&snapshot, &envelope, &mut draft, occurred_at)?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        if transition.session.state.is_terminal() {
            cancel_assistant_snapshot(&mut draft)?;
        }
        let new_operation_id =
            operation_for_effect(&transition.effect).map(|_| DiscoveryOperationId::new());
        let write = DiscoveryTransitionWrite {
            transition,
            draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
            review: review_update,
            new_evidence: Vec::new(),
            new_candidates: Vec::new(),
            approval,
            new_operation_id,
            completed_operation: None,
            prepared_commit,
            provider_graph: None,
            occurred_at,
        };
        self.storage.persist_discovery_transition(&write)?;
        if is_cancel {
            self.settle_prepared_cancellation(session_id)?;
            // A Started operation owns its real cancellation outcome. Do not
            // re-enter the dispatcher without its credential and falsely
            // attest ConfirmedNoExternalEffect while another worker is still
            // in flight. The worker's shared watch token will settle it.
            return self.get(session_id);
        }
        self.drive_nonpersistent(session_id, credential, cancelled)
    }

    /// Collects one new document or one-shot cURL source under the existing
    /// discovery origin and persists only redacted deterministic evidence.
    ///
    /// Collection is bounded. A failed or empty collection leaves the durable
    /// session in `awaiting_more_evidence`. The raw cURL and any extracted
    /// credential are dropped before the action, draft, evidence, or outbox
    /// record is constructed.
    #[allow(clippy::too_many_lines)]
    pub fn supply_additional_evidence(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        source: ProviderDiscoveryAdditionalEvidence,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::AwaitingMoreEvidence {
            return Err(CoreError::invalid(
                "provider discovery is not awaiting more evidence",
            ));
        }
        if snapshot.session.revision != expected_revision {
            return Err(CoreError::invalid(
                "provider discovery revision changed before evidence collection",
            ));
        }
        let (deterministic_source, durable_source) = match source {
            ProviderDiscoveryAdditionalEvidence::DocumentUrl(url) => {
                let origin = origin_from_http_url(&url)?;
                let policy = additional_document_url_policy(&snapshot.session.input, &origin)?;
                let source = DeterministicDiscoverySource::site_with_policy(
                    url.as_str(),
                    policy,
                    DiscoveryFetchBudget::default(),
                )
                .map_err(deterministic_error)?;
                (source, DiscoveryFreshEvidenceSource::DocumentUrl { origin })
            }
            ProviderDiscoveryAdditionalEvidence::Curl(input) => {
                let inspection = inspect_curl(input)
                    .map_err(|_| CoreError::invalid("pasted cURL input was rejected"))?;
                let (evidence, extracted_credential) = inspection.into_parts();
                if extracted_credential.is_some() {
                    drop(extracted_credential);
                    return Err(credential_bearing_curl_requires_handoff());
                }
                let origin = evidence.origin.clone();
                let policy = additional_curl_url_policy(&snapshot.session.input, &origin)?;
                let source =
                    DeterministicDiscoverySource::sanitized_curl_with_policy(evidence, policy)
                        .map_err(deterministic_error)?;
                (
                    source,
                    DiscoveryFreshEvidenceSource::SanitizedCurl { origin },
                )
            }
        };
        let output = self
            .runtime
            .block_on(DeterministicDiscoveryExecutor::new().execute(deterministic_source))
            .map_err(deterministic_error)?;
        let (mut evidence, _) = deterministic_artifacts(&snapshot, &output)?;
        if evidence.is_empty() {
            return Err(CoreError::invalid(
                "additional evidence collection produced no safe evidence",
            ));
        }
        let existing_ids = self
            .storage
            .list_discovery_evidence(session_id, MAX_DISCOVERY_ROWS)?
            .into_iter()
            .map(|record| record.id)
            .collect::<BTreeSet<_>>();
        evidence.retain(|record| !existing_ids.contains(&record.id));
        if evidence.is_empty() {
            return Err(CoreError::invalid(
                "additional evidence collection produced no new safe evidence",
            ));
        }
        let evidence_ids = evidence
            .iter()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();

        let mut draft = hydrate_working_draft(&snapshot)?;
        record_deterministic_assistant_claims(&snapshot, &output, &mut draft)?;
        if draft.assistant.is_some() {
            let mut engine = restored_assistant(&draft)?;
            if engine.state() != AssistantState::AwaitingMoreEvidence {
                return Err(corrupted_assistant_resume_boundary());
            }
            let mut requires_fresh_consent = false;
            for record in &evidence {
                let claims = draft
                    .assistant_evidence_claims
                    .get(&record.id)
                    .cloned()
                    .unwrap_or_default();
                match engine
                    .add_redacted_evidence(redacted_assistant_evidence(record.clone(), claims)?)
                {
                    Ok(()) => {}
                    Err(AssistantError::UnapprovedEvidenceOrigin) => {
                        requires_fresh_consent = true;
                        break;
                    }
                    Err(error) => return Err(assistant_error(error)),
                }
            }
            if requires_fresh_consent {
                // A newly supplied origin is never added to the old egress
                // grant. Rebuild an unconsented assistant from the complete
                // persisted evidence set in the extraction operation below.
                draft.assistant = None;
                draft.assistant_approval_binding = None;
            } else {
                engine
                    .continue_after_more_evidence()
                    .map_err(assistant_error)?;
                synchronize_assistant_snapshot(&mut draft, &engine);
            }
        }
        draft.deterministic = Some(output);
        draft.evidence_ids.extend(evidence_ids.clone());
        draft.evidence_ids.sort();
        draft.evidence_ids.dedup();
        draft.extra_evidence_ids.extend(evidence_ids.clone());
        draft.extra_evidence_ids.sort();
        draft.extra_evidence_ids.dedup();
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            expected_revision,
            ProviderDiscoveryAction::SupplyFreshEvidence {
                evidence_ids,
                source: durable_source,
            },
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        let new_operation_id =
            operation_for_effect(&transition.effect).map(|_| DiscoveryOperationId::new());
        self.storage
            .persist_discovery_transition(&DiscoveryTransitionWrite {
                transition,
                draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
                review: DiscoveryJsonUpdate::Preserve,
                new_evidence: evidence,
                new_candidates: Vec::new(),
                approval: None,
                new_operation_id,
                completed_operation: None,
                prepared_commit: None,
                provider_graph: None,
                occurred_at: Utc::now(),
            })?;
        let (_cancel, cancelled) = watch::channel(false);
        self.drive_nonpersistent(session_id, None, cancelled)
    }

    fn settle_prepared_cancellation(&self, session_id: &DiscoverySessionId) -> CoreResult<()> {
        let snapshot = self.get(session_id)?;
        if !snapshot.session.cancellation_pending {
            return Ok(());
        }
        let Some(operation) = self.storage.get_current_discovery_operation(session_id)? else {
            return Ok(());
        };
        if operation.status != DiscoveryOperationStatus::Prepared
            || operation.kind == DiscoveryOperationKind::Compensation
        {
            return Ok(());
        }
        let mut draft = hydrate_working_draft(&snapshot)?;
        self.persist_operation_completion(
            &snapshot,
            &operation.id,
            &mut draft,
            ProviderDiscoveryAction::Interrupt {
                operation: operation.kind,
                outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
            },
            DurableOperationOutcome::Interrupted,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )
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
                let candidate = self
                    .storage
                    .list_discovery_candidates(session_id, MAX_DISCOVERY_ROWS)?
                    .into_iter()
                    .find(|candidate| candidate.candidate.id == *candidate_id)
                    .ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::NotFound,
                            "setup assistant document candidate was not found",
                            false,
                        )
                    })?;
                let evidence_ids = candidate
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

    fn validate_envelope(envelope: &DiscoveryActionEnvelope) -> CoreResult<()> {
        envelope
            .validate()
            .map_err(|error| CoreError::invalid(format!("invalid discovery action: {error}")))?;
        let expected = canonical_sha256(&envelope.action, "provider discovery action")?;
        if expected != envelope.request_sha256 {
            return Err(CoreError::invalid(
                "provider discovery action hash does not match its canonical payload",
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn prepare_user_action(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        envelope: &DiscoveryActionEnvelope,
        draft: &mut DiscoveryWorkingDraft,
        occurred_at: DateTime<Utc>,
    ) -> CoreResult<(
        Option<DiscoveryApprovalRecord>,
        DiscoveryJsonUpdate<DiscoveryReviewDiff>,
        Option<PreparedDiscoveryCommit>,
    )> {
        let mut review_update = DiscoveryJsonUpdate::Preserve;
        let mut prepared_commit = None;
        let approval = match &envelope.action {
            ProviderDiscoveryAction::SelectTemplate { candidate_id } => {
                select_candidate(self.storage, snapshot, draft, candidate_id, occurred_at)?;
                Some(approval_record(
                    snapshot,
                    approval_proposal_for(
                        &snapshot.session.id,
                        snapshot.session.revision,
                        DiscoveryApprovalGrant::TemplateSelection {
                            candidate_id: candidate_id.clone(),
                        },
                    )?,
                    DiscoveryApprovalDecision::Approved,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::ApproveCredentialOrigin { approval_id } => {
                let proposal = credential_origin_proposal(snapshot, draft)?;
                require_approval_id(approval_id, &proposal)?;
                let connection = draft.connection.as_mut().ok_or_else(|| {
                    CoreError::internal("credential approval has no connection draft")
                })?;
                let template = draft.template.as_ref().ok_or_else(|| {
                    CoreError::internal("credential approval has no template draft")
                })?;
                apply_credential_origin_scope(template, connection);
                draft.credential_approval_id = Some(proposal.id.clone());
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Approved,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::ApproveProbes {
                approval_id,
                approval_grant_sha256,
            } => {
                let proposal = probe_proposal(snapshot, draft)?;
                require_approval_binding(approval_id, approval_grant_sha256, &proposal)?;
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Approved,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::SkipProbes => {
                let proposal = probe_proposal(snapshot, draft)?;
                let review = build_review(draft)?;
                review_update = DiscoveryJsonUpdate::Replace(review);
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Rejected,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::ApproveAssistant {
                approval_id,
                approval_grant_sha256,
            } => {
                let proposal = assistant_proposal(snapshot, draft)?;
                require_approval_binding(approval_id, approval_grant_sha256, &proposal)?;
                grant_assistant_snapshot(snapshot, draft, &proposal.grant)?;
                draft.assistant_approval_binding = Some(DiscoveryApprovalBinding {
                    approval_id: proposal.id.clone(),
                    grant_sha256: proposal.grant_sha256.clone(),
                });
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Approved,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::DeclineAssistant => {
                let proposal = assistant_proposal(snapshot, draft)?;
                draft.assistant_approval_binding = None;
                cancel_assistant_snapshot(draft)?;
                draft.assistant = None;
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Rejected,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::RequestAssistant => {
                initialize_assistant(self.storage, snapshot, draft)?;
                draft.assistant_approval_binding = None;
                None
            }
            ProviderDiscoveryAction::ApproveReview {
                approval_id,
                commit_attempt_id,
                commit_plan_sha256,
                graph_sha256,
            } => {
                let review = snapshot
                    .review
                    .as_ref()
                    .ok_or_else(|| CoreError::internal("review approval has no durable review"))?;
                let current_graph_sha256 = sanitized_graph_sha256(draft)?;
                if review.graph_sha256 != current_graph_sha256
                    || graph_sha256 != &current_graph_sha256
                {
                    return Err(CoreError::invalid(
                        "review approval does not match the current sanitized provider graph",
                    ));
                }
                let expected_attempt = deterministic_commit_attempt_id(
                    &snapshot.session.id,
                    snapshot.session.revision,
                );
                if commit_attempt_id != &expected_attempt {
                    return Err(CoreError::invalid(
                        "review approval commit attempt identifier does not match",
                    ));
                }
                let plan =
                    commit_plan_for(self.storage, snapshot, draft, expected_attempt, review)?;
                let expected_plan_sha256 = canonical_serde_sha256(&plan, "discovery commit plan")?;
                if commit_plan_sha256 != &expected_plan_sha256 {
                    return Err(CoreError::invalid(
                        "review approval commit plan hash does not match",
                    ));
                }
                let proposal = approval_proposal_for(
                    &snapshot.session.id,
                    snapshot.session.revision,
                    DiscoveryApprovalGrant::Review {
                        review_sha256: review.sha256.clone(),
                        graph_sha256: current_graph_sha256,
                    },
                )?;
                require_approval_id(approval_id, &proposal)?;
                let compensation_steps =
                    compensation_recipe(&snapshot.session.id, snapshot.session.revision, &plan);
                prepared_commit = Some(PreparedDiscoveryCommit {
                    plan,
                    plan_sha256: expected_plan_sha256,
                    attempt_number: 1,
                    reuse_existing: false,
                    compensation_steps,
                });
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Approved,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::ResolveUnknownOutcome {
                approval_id,
                resolution,
            } => {
                let operation = snapshot.session.unknown_operation.ok_or_else(|| {
                    CoreError::invalid("discovery has no unknown operation to resolve")
                })?;
                let proposal = approval_proposal_for(
                    &snapshot.session.id,
                    snapshot.session.revision,
                    DiscoveryApprovalGrant::UnknownOutcomeResolution {
                        operation,
                        resolution: resolution.clone(),
                    },
                )?;
                require_approval_id(approval_id, &proposal)?;
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Approved,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::SupplyMoreEvidence { evidence_ids } => {
                let existing = self
                    .storage
                    .list_discovery_evidence(&snapshot.session.id, MAX_DISCOVERY_ROWS)?;
                if evidence_ids
                    .iter()
                    .any(|id| !existing.iter().any(|record| &record.id == id))
                {
                    return Err(CoreError::invalid(
                        "additional evidence must already belong to this discovery session",
                    ));
                }
                draft.extra_evidence_ids.clone_from(evidence_ids);
                draft.assistant = None;
                None
            }
            ProviderDiscoveryAction::RestartInterrupted => {
                if snapshot
                    .session
                    .recovery
                    .as_ref()
                    .is_some_and(|checkpoint| {
                        checkpoint.operation == DiscoveryOperationKind::AtomicCommit
                    })
                {
                    let attempt_id =
                        snapshot.session.commit_attempt_id.as_ref().ok_or_else(|| {
                            CoreError::internal("interrupted commit lost its attempt")
                        })?;
                    let attempt = self.storage.get_discovery_commit_attempt(attempt_id)?;
                    prepared_commit = Some(PreparedDiscoveryCommit {
                        plan: attempt.plan,
                        plan_sha256: attempt.plan_sha256,
                        attempt_number: attempt.attempt_number,
                        reuse_existing: true,
                        compensation_steps: Vec::new(),
                    });
                }
                None
            }
            _ => None,
        };
        Ok((approval, review_update, prepared_commit))
    }

    fn drive_nonpersistent(
        &self,
        session_id: &DiscoverySessionId,
        credential: Option<&str>,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        for _ in 0..MAX_AUTOMATIC_EFFECTS {
            let snapshot = self.get(session_id)?;
            let Some(operation) = self.storage.get_current_discovery_operation(session_id)? else {
                return Ok(snapshot);
            };
            if matches!(
                operation.kind,
                DiscoveryOperationKind::AtomicCommit
                    | DiscoveryOperationKind::Compensation
                    | DiscoveryOperationKind::BuildAssistantManifestDraft
            ) {
                return Ok(snapshot);
            }
            let mut draft = hydrate_working_draft(&snapshot)?;
            let requires_credential = matches!(
                operation.kind,
                DiscoveryOperationKind::ListModels | DiscoveryOperationKind::ProbeCapabilities
            ) && draft
                .template
                .as_ref()
                .is_some_and(|template| template.default_manifest.auth != AuthBinding::None);
            if requires_credential && credential.is_none_or(str::is_empty) {
                self.persist_operation_completion(
                    &snapshot,
                    &operation.id,
                    &mut draft,
                    ProviderDiscoveryAction::Interrupt {
                        operation: operation.kind,
                        outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                    },
                    DurableOperationOutcome::Interrupted,
                    Vec::new(),
                    Vec::new(),
                    DiscoveryJsonUpdate::Preserve,
                )?;
                return self.get(session_id);
            }
            if !self
                .storage
                .mark_discovery_operation_started(&operation.id, Utc::now())?
            {
                return self.get(session_id);
            }
            let completion = match self.execute_nonpersistent_effect(
                &snapshot,
                operation.kind,
                &mut draft,
                credential,
                cancelled.clone(),
            ) {
                Ok(completion) => completion,
                Err(error) => {
                    let (action, outcome) = nonpersistent_failure_action(operation.kind, &error);
                    let completion_snapshot =
                        self.inflight_completion_snapshot(&snapshot, &operation.id)?;
                    self.persist_operation_completion(
                        &completion_snapshot,
                        &operation.id,
                        &mut draft,
                        action,
                        outcome,
                        Vec::new(),
                        Vec::new(),
                        DiscoveryJsonUpdate::Preserve,
                    )?;
                    return self.get(session_id);
                }
            };
            let completion_snapshot =
                self.inflight_completion_snapshot(&snapshot, &operation.id)?;
            self.persist_operation_completion(
                &completion_snapshot,
                &operation.id,
                &mut draft,
                completion.action,
                completion.outcome,
                completion.evidence,
                completion.candidates,
                completion.review,
            )?;
        }
        Err(CoreError::internal(
            "provider discovery exceeded its automatic transition bound",
        ))
    }

    fn inflight_completion_snapshot(
        &self,
        started_snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let latest = self.get(&started_snapshot.session.id)?;
        if latest.session.revision == started_snapshot.session.revision {
            return Ok(latest);
        }
        let current_operation = self
            .storage
            .get_current_discovery_operation(&started_snapshot.session.id)?;
        if latest.session.cancellation_pending
            && latest.session.state == started_snapshot.session.state
            && current_operation.as_ref().is_some_and(|operation| {
                operation.id == *operation_id
                    && operation.status == DiscoveryOperationStatus::Started
            })
        {
            // RequestCancellation deliberately advances the durable revision
            // while the same operation remains active. Complete against that
            // exact cancellation snapshot so the domain transition settles to
            // Cancelled or UnknownOutcome instead of losing the cancellation
            // to a stale-revision write.
            return Ok(latest);
        }
        Err(CoreError::internal(
            "provider discovery changed while its operation was in flight",
        ))
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
    fn persist_operation_completion(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
        draft: &mut DiscoveryWorkingDraft,
        action: ProviderDiscoveryAction,
        outcome: DurableOperationOutcome,
        evidence: Vec<DiscoveryEvidenceRecord>,
        candidates: Vec<StoredDiscoveryCandidate>,
        review: DiscoveryJsonUpdate<DiscoveryReviewDiff>,
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
        self.storage.persist_discovery_transition(&write)?;
        Ok(())
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

    #[allow(clippy::too_many_arguments)]
    fn operation_completion_write(
        snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
        draft: &mut DiscoveryWorkingDraft,
        action: ProviderDiscoveryAction,
        outcome: DurableOperationOutcome,
        evidence: Vec<DiscoveryEvidenceRecord>,
        candidates: Vec<StoredDiscoveryCandidate>,
        review: DiscoveryJsonUpdate<DiscoveryReviewDiff>,
    ) -> CoreResult<DiscoveryTransitionWrite> {
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            action,
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        if transition.session.state.is_terminal() {
            cancel_assistant_snapshot(draft)?;
        }
        let new_operation_id =
            operation_for_effect(&transition.effect).map(|_| DiscoveryOperationId::new());
        Ok(DiscoveryTransitionWrite {
            transition,
            draft: DiscoveryJsonUpdate::Replace(working_draft_value(draft)?),
            review,
            new_evidence: evidence,
            new_candidates: candidates,
            approval: None,
            new_operation_id,
            completed_operation: Some(DiscoveryCompletedOperationWrite {
                id: operation_id.clone(),
                outcome,
            }),
            prepared_commit: None,
            provider_graph: None,
            occurred_at: Utc::now(),
        })
    }
}

struct EffectCompletion {
    action: ProviderDiscoveryAction,
    evidence: Vec<DiscoveryEvidenceRecord>,
    candidates: Vec<StoredDiscoveryCandidate>,
    review: DiscoveryJsonUpdate<DiscoveryReviewDiff>,
    outcome: DurableOperationOutcome,
}

impl EffectCompletion {
    fn simple(action: ProviderDiscoveryAction) -> Self {
        Self {
            action,
            evidence: Vec::new(),
            candidates: Vec::new(),
            review: DiscoveryJsonUpdate::Preserve,
            outcome: DurableOperationOutcome::Succeeded,
        }
    }
}

enum ProbeExecution {
    Completed {
        evidence: Vec<DiscoveryEvidenceRecord>,
    },
    Unknown,
}

fn nonpersistent_failure_action(
    operation: DiscoveryOperationKind,
    error: &CoreError,
) -> (ProviderDiscoveryAction, DurableOperationOutcome) {
    if error.recoverable
        || matches!(
            error.code,
            CoreErrorCode::ProviderAuthFailed
                | CoreErrorCode::ProviderRateLimited
                | CoreErrorCode::ProviderUnavailable
                | CoreErrorCode::NetworkUnavailable
                | CoreErrorCode::Cancelled
                | CoreErrorCode::StorageUnavailable
        )
    {
        (
            ProviderDiscoveryAction::Interrupt {
                operation,
                outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
            },
            DurableOperationOutcome::Interrupted,
        )
    } else {
        (
            ProviderDiscoveryAction::Fail {
                failure: DiscoveryFailure {
                    code: error.code.as_str().to_owned(),
                    message_key: "provider.discovery.operation_failed".to_owned(),
                    recoverable: false,
                },
            },
            DurableOperationOutcome::Failed,
        )
    }
}

fn transition_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!(
        "provider discovery transition was rejected: {error}"
    ))
}

fn working_draft_value(draft: &DiscoveryWorkingDraft) -> CoreResult<Value> {
    serde_json::to_value(draft)
        .map_err(|_| CoreError::internal("provider discovery draft could not be serialized"))
}

fn hydrate_working_draft(snapshot: &DiscoverySessionSnapshot) -> CoreResult<DiscoveryWorkingDraft> {
    let value = snapshot
        .draft_json
        .clone()
        .ok_or_else(|| CoreError::internal("provider discovery draft is missing"))?;
    let draft = serde_json::from_value::<DiscoveryWorkingDraft>(value).map_err(|_| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "provider discovery draft is invalid",
            false,
        )
    })?;
    if draft.schema_version != WORKING_DRAFT_SCHEMA_VERSION {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "provider discovery draft version is unsupported",
            false,
        ));
    }
    Ok(draft)
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

fn operation_for_effect(effect: &DiscoveryEffect) -> Option<DiscoveryOperationKind> {
    match effect {
        DiscoveryEffect::ResolveKnownProvider => Some(DiscoveryOperationKind::ResolveKnownProvider),
        DiscoveryEffect::FetchDocuments => Some(DiscoveryOperationKind::FetchDocuments),
        DiscoveryEffect::ExtractEvidence => Some(DiscoveryOperationKind::ExtractEvidence),
        DiscoveryEffect::BuildDeterministicManifestDraft => {
            Some(DiscoveryOperationKind::BuildDeterministicManifestDraft)
        }
        DiscoveryEffect::BuildAssistantManifestDraft { .. } => {
            Some(DiscoveryOperationKind::BuildAssistantManifestDraft)
        }
        DiscoveryEffect::ValidateManifest => Some(DiscoveryOperationKind::ValidateManifest),
        DiscoveryEffect::ListModels => Some(DiscoveryOperationKind::ListModels),
        DiscoveryEffect::ProbeCapabilities { .. } => {
            Some(DiscoveryOperationKind::ProbeCapabilities)
        }
        DiscoveryEffect::CommitAtomically { .. } => Some(DiscoveryOperationKind::AtomicCommit),
        DiscoveryEffect::RunCompensation { .. } => Some(DiscoveryOperationKind::Compensation),
        DiscoveryEffect::None | DiscoveryEffect::RequestCancellation { .. } => None,
    }
}

fn is_public_discovery_action(action: &ProviderDiscoveryAction) -> bool {
    matches!(
        action,
        ProviderDiscoveryAction::SelectTemplate { .. }
            | ProviderDiscoveryAction::ContinueWithoutTemplate
            | ProviderDiscoveryAction::SupplyMoreEvidence { .. }
            | ProviderDiscoveryAction::RequestAssistant
            | ProviderDiscoveryAction::ApproveAssistant { .. }
            | ProviderDiscoveryAction::DeclineAssistant
            | ProviderDiscoveryAction::ApproveCredentialOrigin { .. }
            | ProviderDiscoveryAction::ApproveProbes { .. }
            | ProviderDiscoveryAction::SkipProbes
            | ProviderDiscoveryAction::ApproveReview { .. }
            | ProviderDiscoveryAction::RestartInterrupted
            | ProviderDiscoveryAction::ResumeCompensation
            | ProviderDiscoveryAction::ResolveUnknownOutcome { .. }
            | ProviderDiscoveryAction::Cancel
    )
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
    let candidate = storage
        .list_discovery_candidates(&snapshot.session.id, MAX_DISCOVERY_ROWS)?
        .into_iter()
        .find(|stored| stored.candidate.id == *candidate_id)
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
    } = candidate.candidate.summary
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

/// Builds a redacted action envelope and hashes only the typed action payload.
pub fn provider_discovery_action_envelope(
    id: DiscoveryActionId,
    expected_revision: u64,
    action: ProviderDiscoveryAction,
) -> CoreResult<DiscoveryActionEnvelope> {
    let request_sha256 = canonical_sha256(&action, "provider discovery action")?;
    Ok(DiscoveryActionEnvelope {
        id,
        expected_revision,
        request_sha256,
        action,
    })
}

impl crate::app::Core {
    pub(crate) fn provider_discovery(&self) -> ProviderDiscoveryOrchestrator<'_> {
        ProviderDiscoveryOrchestrator::new(
            self.storage(),
            self.runtime_handle(),
            self.discovery_recovery_owner(),
        )
    }

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

    pub fn inspect_provider_curl(
        &self,
        input: SecretCurlInput,
        connection_options: ProviderDiscoveryConnectionOptions,
    ) -> CoreResult<ProviderCurlInspection> {
        self.provider_discovery()
            .inspect_curl(input, &connection_options)
    }

    pub fn begin_provider_discovery(
        &self,
        input: SanitizedDiscoveryInput,
        source: ProviderDiscoverySource,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.begin_provider_discovery_with_credential_authority(input, source, None)
    }

    pub fn begin_provider_discovery_with_credential_authority(
        &self,
        input: SanitizedDiscoveryInput,
        source: ProviderDiscoverySource,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().begin_with_credential_authority(
            input,
            source,
            credential_authority,
        )
    }

    pub fn begin_provider_discovery_site(
        &self,
        input: SanitizedDiscoveryInput,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.begin_provider_discovery_site_with_credential_authority(input, None)
    }

    pub fn begin_provider_discovery_site_with_credential_authority(
        &self,
        input: SanitizedDiscoveryInput,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().begin_with_credential_authority(
            input,
            ProviderDiscoverySource::site(),
            credential_authority,
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

    pub fn begin_provider_discovery_curl(
        &self,
        input: ProviderDiscoveryCurlInput,
        curl: SecretCurlInput,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.begin_provider_discovery_curl_with_credential_authority(input, curl, None)
    }

    pub fn begin_provider_discovery_curl_with_credential_authority(
        &self,
        input: ProviderDiscoveryCurlInput,
        curl: SecretCurlInput,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .begin_curl_with_credential_authority(input, curl, credential_authority)
    }

    pub fn list_provider_discoveries(
        &self,
        limit: u32,
    ) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        self.provider_discovery().list(limit)
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

    pub fn get_provider_discovery(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().get(session_id)
    }

    pub fn list_provider_discovery_evidence(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Vec<DiscoveryEvidenceRecord>> {
        self.provider_discovery().evidence(session_id)
    }

    pub fn list_provider_discovery_approvals(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Vec<DiscoveryApprovalRecord>> {
        self.provider_discovery().approvals(session_id)
    }

    pub fn get_provider_discovery_review(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<DiscoveryReviewDiff>> {
        self.provider_discovery().review(session_id)
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

    pub fn continue_provider_discovery(
        &self,
        session_id: &DiscoverySessionId,
        envelope: DiscoveryActionEnvelope,
        credential: Option<&str>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        if matches!(&envelope.action, ProviderDiscoveryAction::Cancel)
            && let Some(physical_authority_id) = self.prepared_discovery_credential_reservation_id(
                session_id,
                envelope.expected_revision,
            )?
        {
            self.forget_discovery_credential_reservation(&physical_authority_id)?;
        }
        self.provider_discovery()
            .continue_discovery(session_id, envelope, credential)
    }

    pub fn continue_provider_discovery_with_cancellation(
        &self,
        session_id: &DiscoverySessionId,
        envelope: DiscoveryActionEnvelope,
        credential: Option<&str>,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        if matches!(&envelope.action, ProviderDiscoveryAction::Cancel)
            && let Some(physical_authority_id) = self.prepared_discovery_credential_reservation_id(
                session_id,
                envelope.expected_revision,
            )?
        {
            // Cancellation abandons a Prepared reservation. Consume the
            // process-local capability first so any later transition failure
            // remains fail-closed and cannot make that slot reusable.
            self.forget_discovery_credential_reservation(&physical_authority_id)?;
        }
        self.provider_discovery()
            .continue_discovery_with_cancellation(session_id, envelope, credential, cancelled)
    }

    pub fn supply_provider_discovery_evidence(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        source: ProviderDiscoveryAdditionalEvidence,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .supply_additional_evidence(session_id, expected_revision, source)
    }

    pub fn cancel_provider_discovery(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            expected_revision,
            ProviderDiscoveryAction::Cancel,
        )?;
        self.continue_provider_discovery(session_id, envelope, None)
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

    pub fn poll_provider_discovery_events(
        &self,
        limit: u32,
        available_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
        self.provider_discovery().poll_outbox(limit, available_at)
    }

    pub fn poll_provider_discovery_events_for_session(
        &self,
        session_id: &DiscoverySessionId,
        limit: u32,
        available_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
        self.provider_discovery()
            .poll_outbox_for_session(session_id, limit, available_at)
    }

    pub fn ack_provider_discovery_event(
        &self,
        event_id: &DiscoveryEventId,
        delivered_at: DateTime<Utc>,
    ) -> CoreResult<bool> {
        self.provider_discovery().ack_outbox(event_id, delivered_at)
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

fn discovery_url_policy(options: &ProviderDiscoveryConnectionOptions) -> CoreResult<UrlPolicy> {
    require_active_discovery_network_authority(options, Utc::now())?;
    unissued_discovery_url_policy(options)
}

/// Builds a policy only for pre-session cURL parsing. It never authorizes a
/// network effect; durable sessions receive their server-issued timestamp at
/// `begin_with_credential_authority` before any effect is driven.
fn unissued_discovery_url_policy(
    options: &ProviderDiscoveryConnectionOptions,
) -> CoreResult<UrlPolicy> {
    options
        .validate()
        .map_err(|error| CoreError::invalid(format!("invalid connection options: {error}")))?;
    match (
        options.network_mode,
        options.local_network_approval.as_ref(),
    ) {
        (ProviderNetworkMode::Public, None) => Ok(UrlPolicy::public()),
        (ProviderNetworkMode::LocalLoopback, None) => Ok(UrlPolicy::local_loopback()),
        (ProviderNetworkMode::ApprovedLocalNetwork, Some(approval)) => {
            let approval =
                ApprovedLocalNetworkOrigin::new(approval.origin.as_str(), &approval.addresses)
                    .map_err(|_| {
                        CoreError::invalid("approved local-network policy was rejected")
                    })?;
            Ok(UrlPolicy::approved_local_network(approval))
        }
        _ => Err(CoreError::invalid(
            "connection network mode and local-network approval do not match",
        )),
    }
}

fn additional_document_url_policy(
    input: &SanitizedDiscoveryInput,
    source_origin: &CanonicalOrigin,
) -> CoreResult<UrlPolicy> {
    match input.connection_options.network_mode {
        ProviderNetworkMode::Public => discovery_url_policy(&input.connection_options),
        ProviderNetworkMode::LocalLoopback => {
            require_discovery_site_origin(input, source_origin)?;
            discovery_url_policy(&input.connection_options)
        }
        ProviderNetworkMode::ApprovedLocalNetwork => Err(approved_lan_web_discovery_disabled()),
    }
}

fn additional_curl_url_policy(
    input: &SanitizedDiscoveryInput,
    source_origin: &CanonicalOrigin,
) -> CoreResult<UrlPolicy> {
    match input.connection_options.network_mode {
        ProviderNetworkMode::Public => discovery_url_policy(&input.connection_options),
        ProviderNetworkMode::LocalLoopback => {
            require_discovery_site_origin(input, source_origin)?;
            discovery_url_policy(&input.connection_options)
        }
        ProviderNetworkMode::ApprovedLocalNetwork => {
            let approved_origin = input
                .connection_options
                .local_network_approval
                .as_ref()
                .map(|approval| &approval.origin)
                .ok_or_else(|| CoreError::invalid("local-network approval is missing"))?;
            if source_origin != approved_origin {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "cURL origin is outside the approved local-network origin",
                    false,
                ));
            }
            discovery_url_policy(&input.connection_options)
        }
    }
}

fn require_discovery_site_origin(
    input: &SanitizedDiscoveryInput,
    source_origin: &CanonicalOrigin,
) -> CoreResult<()> {
    let site_origin = origin_from_http_url(&input.site_url)?;
    if source_origin == &site_origin {
        Ok(())
    } else {
        Err(CoreError::new(
            CoreErrorCode::PermissionDenied,
            "local discovery evidence must use the exact discovery site origin",
            false,
        ))
    }
}

fn approved_lan_web_discovery_disabled() -> CoreError {
    CoreError::new(
        CoreErrorCode::PermissionDenied,
        "approved local-network web discovery is disabled without a separate network-read approval",
        false,
    )
}

fn credential_bearing_curl_requires_handoff() -> CoreError {
    CoreError::invalid(
        "credential-bearing cURL must be inspected first and only its redacted cURL submitted after native-vault handoff",
    )
}

fn canonical_sha256<T: Serialize>(value: &T, label: &str) -> CoreResult<String> {
    let value = serde_json::to_value(value)
        .map_err(|_| CoreError::internal(format!("{label} could not be serialized")))?;
    let mut canonical = String::new();
    write_canonical_json(&value, &mut canonical)?;
    Ok(sha256_hex(canonical.as_bytes()))
}

fn write_canonical_json(value: &Value, output: &mut String) -> CoreResult<()> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => output.push_str(
            &serde_json::to_string(value)
                .map_err(|_| CoreError::internal("JSON string could not be serialized"))?,
        ),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(
                    &serde_json::to_string(key)
                        .map_err(|_| CoreError::internal("JSON key could not be serialized"))?,
                );
                output.push(':');
                write_canonical_json(
                    values
                        .get(key)
                        .ok_or_else(|| CoreError::internal("canonical JSON key disappeared"))?,
                    output,
                )?;
            }
            output.push('}');
        }
    }
    Ok(())
}

fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

/// Bounded project-owned fixtures for downstream Shell adapter tests.
#[cfg(feature = "test-support")]
pub mod test_support {
    use super::{
        AdapterRegistry, BuiltInTemplateId, CoreError, CoreResult, CredentialRef,
        DiscoveryActionId, DiscoveryApprovalId, DiscoveryCandidateSummary, DiscoveryJsonUpdate,
        DiscoveryOperationId, DiscoveryOperationKind, DiscoverySessionSnapshot,
        DiscoveryTransitionWrite, DurableOperationOutcome, HttpUrl, ProviderConnectionId,
        ProviderDiscoveryAction, ProviderDiscoveryConnectionOptions,
        ProviderDiscoveryCredentialInstallContext, ProviderDiscoveryCredentialLeaseContext,
        SanitizedDiscoveryInput, Utc, apply_listed_models_to_draft, hydrate_working_draft,
        model_candidates, operation_for_effect, provider_discovery_action_envelope,
        transition_error, working_draft_value,
    };

    /// Exact non-secret contexts for one synthetic Started discovery install.
    pub struct SyntheticStartedCredentialInstall {
        pub install: ProviderDiscoveryCredentialInstallContext,
        pub lease: ProviderDiscoveryCredentialLeaseContext,
    }

    /// Seeds one fixed OpenRouter-shaped commit without network access and
    /// advances its credential WAL through the exact native Started cutpoint.
    pub fn seed_synthetic_started_credential_install(
        core: &crate::Core,
        connection_id: &str,
    ) -> CoreResult<SyntheticStartedCredentialInstall> {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)?;
        let connection_id = ProviderConnectionId::from(connection_id);
        let selecting = core.begin_provider_discovery_known(
            SanitizedDiscoveryInput {
                connection_id: connection_id.clone(),
                display_name: "Synthetic Shell direct-capture fixture".to_owned(),
                site_url: HttpUrl::parse("https://docs.openrouter.example/")
                    .map_err(CoreError::invalid)?,
                docs_url: None,
                credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
                preferred_assistant: None,
                connection_options: ProviderDiscoveryConnectionOptions::default(),
                supplied_evidence_ids: Vec::new(),
            },
            template.id.clone(),
        )?;
        let candidate = core
            .list_provider_discovery_candidates(&selecting.session.id)?
            .into_iter()
            .find(|candidate| {
                matches!(
                    &candidate.candidate.summary,
                    DiscoveryCandidateSummary::ProviderTemplate {
                        template_id,
                        template_version,
                    } if template_id == &template.id
                        && *template_version == template.manifest_version
                )
            })
            .ok_or_else(|| CoreError::internal("synthetic discovery candidate is missing"))?;
        let selected = core.continue_provider_discovery(
            &selecting.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                selecting.session.revision,
                ProviderDiscoveryAction::SelectTemplate {
                    candidate_id: candidate.candidate.id,
                },
            )?,
            None,
        )?;
        let approval = core
            .get_provider_discovery_approval_proposal(&selected.session.id)?
            .ok_or_else(|| CoreError::internal("synthetic credential approval is missing"))?;
        let listed = approve_and_seed_synthetic_listing(core, &selected, approval.id)?;
        let reviewed = core.continue_provider_discovery(
            &listed.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                listed.session.revision,
                ProviderDiscoveryAction::SkipProbes,
            )?,
            None,
        )?;
        let lease = core.get_provider_discovery_credential_lease_context(&reviewed.session.id)?;
        let proposal = core
            .get_provider_discovery_review_proposal(&reviewed.session.id)?
            .ok_or_else(|| CoreError::internal("synthetic review proposal is missing"))?;
        let committing = core.continue_provider_discovery(
            &reviewed.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                reviewed.session.revision,
                ProviderDiscoveryAction::ApproveReview {
                    approval_id: proposal.approval.id,
                    commit_attempt_id: proposal.commit_attempt_id,
                    commit_plan_sha256: proposal.commit_plan_sha256,
                    graph_sha256: proposal.review.graph_sha256,
                },
            )?,
            None,
        )?;
        let prepared =
            core.get_provider_discovery_credential_install_context(&committing.session.id)?;
        let reserved = core.reserve_provider_discovery_credential_install(
            &prepared.session_id,
            prepared.session_revision,
            &prepared.operation_id,
            &prepared.commit_attempt_id,
            &prepared.commit_plan_sha256,
        )?;
        let reservation_id = reserved
            .native_execution_reservation_id
            .as_deref()
            .ok_or_else(|| CoreError::internal("synthetic reservation is missing"))?;
        let install = core.start_provider_discovery_credential_install(
            &reserved.session_id,
            reserved.session_revision,
            &reserved.operation_id,
            &reserved.commit_attempt_id,
            &reserved.commit_plan_sha256,
            reservation_id,
        )?;
        Ok(SyntheticStartedCredentialInstall { install, lease })
    }

    fn approve_and_seed_synthetic_listing(
        core: &crate::Core,
        snapshot: &DiscoverySessionSnapshot,
        approval_id: DiscoveryApprovalId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let orchestrator = core.provider_discovery();
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::ApproveCredentialOrigin { approval_id },
        )?;
        let mut draft = hydrate_working_draft(snapshot)?;
        let occurred_at = Utc::now();
        let (approval, review, prepared_commit) =
            orchestrator.prepare_user_action(snapshot, &envelope, &mut draft, occurred_at)?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        let new_operation_id =
            operation_for_effect(&transition.effect).map(|_| DiscoveryOperationId::new());
        orchestrator
            .storage
            .persist_discovery_transition(&DiscoveryTransitionWrite {
                transition,
                draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
                review,
                new_evidence: Vec::new(),
                new_candidates: Vec::new(),
                approval,
                new_operation_id,
                completed_operation: None,
                prepared_commit,
                provider_graph: None,
                occurred_at,
            })?;
        let listing = orchestrator.get(&snapshot.session.id)?;
        let operation = orchestrator
            .storage
            .get_current_discovery_operation(&snapshot.session.id)?
            .ok_or_else(|| CoreError::internal("synthetic model listing is missing"))?;
        if operation.kind != DiscoveryOperationKind::ListModels
            || !orchestrator
                .storage
                .mark_discovery_operation_started(&operation.id, Utc::now())?
        {
            return Err(CoreError::internal(
                "synthetic model listing did not reach its start cutpoint",
            ));
        }
        let mut draft = hydrate_working_draft(&listing)?;
        apply_listed_models_to_draft(&mut draft, &[synthetic_listed_model()], Utc::now())?;
        draft.probe_route_ids = draft.routes.iter().map(|route| route.id.clone()).collect();
        let candidates = model_candidates(&listing, &draft)?;
        orchestrator.persist_operation_completion(
            &listing,
            &operation.id,
            &mut draft,
            ProviderDiscoveryAction::ModelsListed {
                model_count: 1,
                probe_candidate_count: 1,
            },
            DurableOperationOutcome::Succeeded,
            Vec::new(),
            candidates,
            DiscoveryJsonUpdate::Preserve,
        )?;
        orchestrator.get(&snapshot.session.id)
    }

    fn synthetic_listed_model() -> lorepia_providers::ListedModel {
        lorepia_providers::ListedModel {
            model_id: "openai/synthetic-shell-direct-capture".to_owned(),
            display_name: Some("Synthetic Shell direct capture".to_owned()),
            max_input_tokens: Some(4_096),
            max_output_tokens: Some(1_024),
            supported_generation_methods: Vec::new(),
            capabilities: lorepia_providers::ListedModelCapabilities {
                supported: Vec::new(),
                parameters: lorepia_providers::OpenRouterSupportedParameterSupport::Exact(
                    Vec::new(),
                ),
                reasoning: None,
            },
            source: lorepia_providers::ModelRecordSource::ProviderApi,
            availability: lorepia_domain::ModelAvailability::Available,
        }
    }
}

#[cfg(test)]
mod policy_tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use std::{
        io::Read,
        net::{IpAddr, TcpListener, TcpStream},
        sync::mpsc as std_mpsc,
        thread,
    };

    use lorepia_domain::{
        EndpointPath, GenerationUsage, ModelAvailability, ModelMetadataSource, ModelRouteConfig,
        ProviderCapabilities, ProviderConnectionDraft, ProviderProfile,
    };
    use lorepia_providers::setup_assistant::{
        AssistantManifestDraft, AssistantTurn, ConfidenceLevel, FieldConfidence,
        FieldEvidenceMapping,
    };
    use lorepia_storage::{ProviderCredentialObservedStatus, ProviderCredentialOperationKind};
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    mod schema_fixture {
        include!("provider_discovery/schema_fixture.rs");
    }

    struct ConstrainedAssistantCaptureProvider {
        plain_generate_called: Arc<AtomicBool>,
        captured_bodies: Arc<Mutex<Vec<(ApiFamily, Value)>>>,
        response: String,
    }

    fn read_probe_request_headers(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set probe request timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).expect("read probe request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8(request).expect("probe request is UTF-8")
    }

    fn spawn_stalling_probe_provider() -> (
        String,
        std_mpsc::Receiver<String>,
        std_mpsc::Sender<()>,
        std_mpsc::Receiver<bool>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind probe provider");
        let address = listener.local_addr().expect("probe provider address");
        let (request_sender, request_receiver) = std_mpsc::channel();
        let (release_sender, release_receiver) = std_mpsc::channel();
        let (later_dispatch_sender, later_dispatch_receiver) = std_mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut first, _) = listener.accept().expect("accept first probe request");
            request_sender
                .send(read_probe_request_headers(&mut first))
                .expect("report first probe request");
            release_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("release first probe request");
            drop(first);

            listener
                .set_nonblocking(true)
                .expect("make probe listener nonblocking");
            let deadline = std::time::Instant::now() + Duration::from_millis(750);
            let mut later_dispatch = false;
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((_stream, _)) => {
                        later_dispatch = true;
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept later probe request: {error}"),
                }
            }
            later_dispatch_sender
                .send(later_dispatch)
                .expect("report later probe dispatch");
        });
        (
            format!("http://{address}"),
            request_receiver,
            release_sender,
            later_dispatch_receiver,
            handle,
        )
    }

    #[async_trait::async_trait]
    impl Provider for ConstrainedAssistantCaptureProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: false,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            _sink: lorepia_providers::ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            self.plain_generate_called.store(true, Ordering::SeqCst);
            Err(CoreError::internal(
                "bare setup-assistant generation must never be called",
            ))
        }

        async fn generate_with_internal_plan(
            &self,
            request: GenerationRequest,
            _credential: Option<&str>,
            sink: lorepia_providers::ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
            request_plan: lorepia_providers::parameter_mapping::ProviderRequestPlan,
        ) -> CoreResult<GenerationUsage> {
            let mut body = json!({"model": request.model});
            request_plan
                .apply_to_body(&mut body)
                .map_err(|error| CoreError::invalid(error.to_string()))?;
            self.captured_bodies
                .lock()
                .expect("capture setup-assistant body")
                .push((request_plan.family(), body));
            sink.send(ProviderEvent::TextDelta(self.response.clone()))
                .await
                .map_err(|_| CoreError::internal("setup-assistant event receiver closed"))?;
            Ok(GenerationUsage {
                input_tokens: Some(8),
                cached_read_tokens: None,
                cached_write_tokens: None,
                output_tokens: Some(8),
                reasoning_tokens: None,
                tool_tokens: None,
                provider_raw_summary: None,
            })
        }
    }

    struct PlainOnlyAssistantProvider {
        plain_generate_called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl Provider for PlainOnlyAssistantProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: false,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            _sink: lorepia_providers::ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            self.plain_generate_called.store(true, Ordering::SeqCst);
            Ok(GenerationUsage {
                input_tokens: None,
                cached_read_tokens: None,
                cached_write_tokens: None,
                output_tokens: None,
                reasoning_tokens: None,
                tool_tokens: None,
                provider_raw_summary: None,
            })
        }
    }

    fn assert_file_tree_omits(root: &std::path::Path, forbidden: &[u8]) {
        for entry in std::fs::read_dir(root).expect("read test data root") {
            let entry = entry.expect("read test data entry");
            let path = entry.path();
            if path.is_dir() {
                assert_file_tree_omits(&path, forbidden);
            } else {
                let bytes = std::fs::read(&path).expect("read test data file");
                assert!(
                    !bytes
                        .windows(forbidden.len())
                        .any(|window| window == forbidden),
                    "forbidden provider output persisted in {}",
                    path.display()
                );
            }
        }
    }

    fn input_with_options(
        site_url: &str,
        connection_options: ProviderDiscoveryConnectionOptions,
    ) -> SanitizedDiscoveryInput {
        SanitizedDiscoveryInput {
            connection_id: ProviderConnectionId::from("policy-test-connection"),
            display_name: "Policy test provider".to_owned(),
            site_url: HttpUrl::parse(site_url).unwrap(),
            docs_url: None,
            credential_ref: None,
            preferred_assistant: None,
            connection_options,
            supplied_evidence_ids: Vec::new(),
        }
    }

    #[test]
    fn signed_discovery_template_without_current_operational_authority_fails_closed() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
        let now = Utc::now();
        let mut template =
            AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter).unwrap();
        template.source = TemplateSource::SignedCatalog;
        template.manifest_version += 1;
        let authority = DiscoveryCatalogAuthorityBinding::new(
            1,
            &template,
            now + chrono::Duration::minutes(10),
        )
        .unwrap();
        let mut draft = DiscoveryWorkingDraft::new(DiscoverySourceIntent::KnownProvider {
            template_id: template.id.clone(),
        });
        draft.template = Some(template);
        draft.catalog_authority = Some(authority);

        let error = revalidate_discovery_catalog_authority(core.storage(), &draft, now)
            .expect_err("inactive signed template must not retain discovery authority");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
    }

    fn credential_commit_confirmation(
        context: &ProviderDiscoveryCredentialInstallContext,
    ) -> ProviderDiscoveryCredentialCommitConfirmation {
        ProviderDiscoveryCredentialCommitConfirmation::try_from(context)
            .expect("started credential install context has a physical execution authority")
    }

    fn reserve_credential_install(
        core: &crate::Core,
        prepared: &ProviderDiscoveryCredentialInstallContext,
    ) -> ProviderDiscoveryCredentialInstallContext {
        let reserved = core
            .reserve_provider_discovery_credential_install(
                &prepared.session_id,
                prepared.session_revision,
                &prepared.operation_id,
                &prepared.commit_attempt_id,
                &prepared.commit_plan_sha256,
            )
            .expect("reserve exact physical credential execution");
        assert_eq!(
            reserved.operation_status,
            DiscoveryOperationStatus::Prepared
        );
        assert!(reserved.native_execution_reservation_id.is_some());
        assert_eq!(reserved.native_execution_id, None);
        assert!(
            ProviderDiscoveryCredentialCommitConfirmation::try_from(&reserved).is_err(),
            "a reservation is not native store or commit authority"
        );
        reserved
    }

    fn start_reserved_credential_install(
        core: &crate::Core,
        reserved: &ProviderDiscoveryCredentialInstallContext,
    ) -> ProviderDiscoveryCredentialInstallContext {
        let reservation_id = reserved
            .native_execution_reservation_id
            .as_deref()
            .expect("reserved physical credential execution");
        let started = core
            .start_provider_discovery_credential_install(
                &reserved.session_id,
                reserved.session_revision,
                &reserved.operation_id,
                &reserved.commit_attempt_id,
                &reserved.commit_plan_sha256,
                reservation_id,
            )
            .expect("start exact reserved physical credential execution");
        assert_eq!(started.operation_status, DiscoveryOperationStatus::Started);
        assert_eq!(
            started.native_execution_reservation_id.as_deref(),
            started.native_execution_id.as_deref()
        );
        assert!(started.native_execution_id.is_some());
        started
    }

    fn reserve_and_start_credential_install(
        core: &crate::Core,
        prepared: &ProviderDiscoveryCredentialInstallContext,
    ) -> ProviderDiscoveryCredentialInstallContext {
        let reserved = reserve_credential_install(core, prepared);
        start_reserved_credential_install(core, &reserved)
    }

    fn native_execution_id(context: &ProviderDiscoveryCredentialInstallContext) -> &str {
        context
            .native_execution_id
            .as_deref()
            .expect("started credential install has native execution authority")
    }

    fn persist_unsettled_credential_cancel(
        core: &crate::Core,
        snapshot: &DiscoverySessionSnapshot,
    ) -> DiscoverySessionSnapshot {
        let orchestrator = core.provider_discovery();
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::Cancel,
        )
        .expect("build exact cancellation action");
        let mut draft = hydrate_working_draft(snapshot).expect("hydrate cancellation draft");
        let occurred_at = Utc::now();
        let (approval, review, prepared_commit) = orchestrator
            .prepare_user_action(snapshot, &envelope, &mut draft, occurred_at)
            .expect("prepare exact cancellation action");
        let transition = snapshot
            .session
            .apply(&envelope)
            .expect("apply exact cancellation action");
        orchestrator
            .storage
            .persist_discovery_transition(&DiscoveryTransitionWrite {
                transition,
                draft: DiscoveryJsonUpdate::Replace(
                    working_draft_value(&draft).expect("serialize cancellation draft"),
                ),
                review,
                new_evidence: Vec::new(),
                new_candidates: Vec::new(),
                approval,
                new_operation_id: None,
                completed_operation: None,
                prepared_commit,
                provider_graph: None,
                occurred_at,
            })
            .expect("persist cancellation before prepared-operation settlement");
        orchestrator
            .get(&snapshot.session.id)
            .expect("reload unsettled cancellation")
    }

    fn seed_started_cancellation_for_tamper(
        root: &std::path::Path,
        connection_id: &str,
    ) -> DiscoverySessionId {
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(root),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open tamper fixture Core");
        let committing = prepare_no_network_credential_commit(&core, connection_id);
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("load tamper fixture credential operation");
        let started = reserve_and_start_credential_install(&core, &prepared);
        core.cancel_provider_discovery(&committing.session.id, started.session_revision)
            .expect("persist tamper fixture cancellation");
        drop(core);
        checkpoint_test_database(&active_test_database_path(root));
        committing.session.id
    }

    fn active_test_database_path(root: &std::path::Path) -> std::path::PathBuf {
        let cutover = root.join("db/schema-cutover");
        let (_, relative) = std::fs::read_dir(cutover)
            .expect("read committed database generations")
            .filter_map(Result::ok)
            .filter(|entry| entry.path().join("generation-committed.json").is_file())
            .map(|entry| {
                let manifest = serde_json::from_slice::<serde_json::Value>(
                    &std::fs::read(entry.path().join("generation-manifest.json"))
                        .expect("read generation manifest"),
                )
                .expect("parse generation manifest");
                let sequence = manifest["activation_sequence"]
                    .as_u64()
                    .expect("generation activation sequence");
                let relative = manifest["active_database_relative_path"]
                    .as_str()
                    .expect("active database relative path")
                    .to_owned();
                (sequence, relative)
            })
            .max_by_key(|(sequence, _)| *sequence)
            .expect("at least one committed database generation");
        root.join(relative)
    }

    fn open_core_after_drop(
        data_root: &std::path::Path,
        recovery_owner: crate::DiscoveryRecoveryOwner,
    ) -> crate::Core {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match crate::Core::open_with_discovery_recovery_owner(
                crate::CoreConfig::new(data_root),
                recovery_owner,
            ) {
                Ok(core) => return core,
                Err(error)
                    if error.code == CoreErrorCode::StorageUnavailable
                        && error.message
                            == "data root is already owned by another LorePia process"
                        && std::time::Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("open Core after prior owner drop: {error:?}"),
            }
        }
    }

    fn checkpoint_test_database(database: &std::path::Path) {
        let connection = rusqlite::Connection::open(database).expect("open test database");
        let _: (i64, i64, i64) = connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .expect("checkpoint test database");
    }

    fn restore_test_database(database: &std::path::Path, backup: &std::path::Path) {
        for sidecar in [
            database.with_extension("sqlite3-wal"),
            database.with_extension("sqlite3-shm"),
        ] {
            if sidecar.exists() {
                std::fs::remove_file(sidecar).expect("remove rolled-forward SQLite sidecar");
            }
        }
        std::fs::copy(backup, database).expect("restore prepared test database");
    }

    fn restore_schema36_trigger(
        connection: &rusqlite::Connection,
        migration: &str,
        trigger_name: &str,
    ) {
        connection
            .execute_batch(&format!("DROP TRIGGER {trigger_name};"))
            .unwrap_or_else(|error| panic!("drop schema-37 trigger {trigger_name}: {error}"));
        let marker = format!("CREATE TRIGGER {trigger_name}\n");
        let start = migration
            .find(&marker)
            .unwrap_or_else(|| panic!("find schema-36 trigger {trigger_name}"));
        let tail = &migration[start..];
        let end = tail.find("\nEND;").map_or_else(
            || panic!("find end of schema-36 trigger {trigger_name}"),
            |offset| offset + "\nEND;".len(),
        );
        connection
            .execute_batch(&tail[..end])
            .unwrap_or_else(|error| panic!("restore schema-36 trigger {trigger_name}: {error}"));
    }

    // Keep the complete schema downgrade in one fixture transaction so callers cannot
    // accidentally observe or reuse a partially reversed credential schema.
    #[allow(clippy::too_many_lines)]
    fn reverse_schema37_credential_migration(database: &std::path::Path) {
        const MIGRATION_0027: &str = include_str!(
            "../../storage/migrations/0027_provider_discovery_native_attestations.sql"
        );
        const MIGRATION_0037: &str =
            include_str!("../../storage/migrations/0037_provider_credential_operations.sql");

        let connection = rusqlite::Connection::open(database).expect("open current database");
        connection
            .execute_batch("PRAGMA foreign_keys = OFF; BEGIN IMMEDIATE;")
            .expect("begin exact schema-37 inverse");
        schema_fixture::drop_post_schema_37_additive_migrations(&connection);
        for trigger_name in [
            "provider_discovery_native_no_effect_attestation_binding",
            "provider_discovery_operation_legal_transition",
        ] {
            restore_schema36_trigger(&connection, MIGRATION_0027, trigger_name);
        }
        let replaced_objects = MIGRATION_0037
            .lines()
            .filter_map(|line| {
                let mut tokens = line.split_ascii_whitespace();
                (tokens.next() == Some("DROP")).then_some(())?;
                Some((tokens.next()?, tokens.next()?.trim_end_matches(';')))
            })
            .collect::<Vec<_>>();
        let created_objects = MIGRATION_0037
            .lines()
            .filter_map(|line| {
                let mut tokens = line.split_ascii_whitespace();
                (tokens.next() == Some("CREATE")).then_some(())?;
                let object_type = tokens.next()?;
                let (object_type, name) = if object_type == "UNIQUE" {
                    (tokens.next()?, tokens.next()?)
                } else {
                    (object_type, tokens.next()?)
                };
                let name = name.trim_end_matches(';');
                (!replaced_objects.contains(&(object_type, name))).then_some((object_type, name))
            })
            .collect::<Vec<_>>();
        assert!(
            created_objects.contains(&(
                "TABLE",
                "provider_discovery_native_credential_legacy_started_cutoff_snapshots"
            )),
            "schema-37 inverse must discover the legacy cutoff table"
        );
        for object_type in ["VIEW", "TRIGGER", "INDEX", "TABLE"] {
            for (_, name) in created_objects
                .iter()
                .rev()
                .filter(|(candidate_type, _)| *candidate_type == object_type)
            {
                connection
                    .execute(&format!("DROP {object_type} \"{name}\""), [])
                    .unwrap_or_else(|error| panic!("drop schema-37 {object_type} {name}: {error}"));
            }
        }
        assert_eq!(
            connection
                .execute("DELETE FROM schema_migrations WHERE version = 37", [])
                .expect("remove schema-37 migration registry row"),
            1
        );
        connection
            .execute_batch("COMMIT; PRAGMA foreign_keys = ON;")
            .expect("commit exact schema-37 inverse");
        assert_eq!(
            connection
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get::<_, u32>(0)
                })
                .expect("read simulated schema version"),
            36
        );
        assert_eq!(
            connection
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .expect("validate simulated schema-36 database"),
            "ok"
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get::<_, u64>(0)
                })
                .expect("validate simulated schema-36 foreign keys"),
            0
        );
    }

    fn approved_lan_options() -> ProviderDiscoveryConnectionOptions {
        ProviderDiscoveryConnectionOptions {
            network_mode: ProviderNetworkMode::ApprovedLocalNetwork,
            local_network_approval: Some(ProviderLocalNetworkApproval {
                origin: CanonicalOrigin::parse("http://models.lan:8080").unwrap(),
                addresses: vec!["192.168.10.20".parse::<IpAddr>().unwrap()],
            }),
            local_network_approved_at: Some(Utc::now()),
            ..ProviderDiscoveryConnectionOptions::default()
        }
    }

    fn probe_route(id: &str, endpoint_path: &str) -> ModelRoute {
        let now = Utc::now();
        ModelRoute {
            id: ModelRouteId::from(id),
            connection_id: ProviderConnectionId::from("probe-connection"),
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: format!("{id}-model"),
            display_name: None,
            route_config: ModelRouteConfig {
                endpoint_path: Some(EndpointPath::parse(endpoint_path).expect("endpoint path")),
                ..ModelRouteConfig::default()
            },
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::Legacy,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: now,
            last_seen_at: Some(now),
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn cached_credential_authority_cannot_start_discovery_after_terminal_removal() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
        let connection_id = ProviderConnectionId::from("core-stale-discovery-authority");
        core.storage()
            .save_provider_profile(&ProviderProfile {
                id: connection_id.as_str().to_owned(),
                display_name: "Core stale discovery authority".to_owned(),
                base_url: "https://provider.example/v1".to_owned(),
                model: "synthetic".to_owned(),
                timeout_seconds: 30,
            })
            .expect("save credential-bound provider");
        let install_authority = core
            .storage()
            .propose_provider_credential_install_authority(&connection_id)
            .expect("propose credential install authority");
        let install = core
            .storage()
            .prepare_provider_credential_operation_with_install_authority(
                &connection_id,
                ProviderCredentialOperationKind::Install,
                ProviderCredentialObservedStatus::Missing,
                Some(&install_authority),
            )
            .expect("prepare credential install");
        core.storage()
            .start_provider_credential_operation(&install.plan.operation_id, &install.plan_sha256)
            .expect("start credential install");
        core.storage()
            .finish_provider_credential_operation(
                &install.plan.operation_id,
                &install.plan_sha256,
                ProviderCredentialObservedStatus::Available,
            )
            .expect("finish credential install");
        let cached_authority = core
            .storage()
            .ensure_provider_credential_access_settled(&connection_id)
            .expect("capture credential read authority");
        let removal = core
            .storage()
            .prepare_provider_credential_operation(
                &connection_id,
                ProviderCredentialOperationKind::RemoveCredential,
                ProviderCredentialObservedStatus::Available,
            )
            .expect("prepare credential removal");
        core.storage()
            .start_provider_credential_operation(&removal.plan.operation_id, &removal.plan_sha256)
            .expect("start credential removal");
        core.storage()
            .finish_provider_credential_operation(
                &removal.plan.operation_id,
                &removal.plan_sha256,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("terminalize credential removal");

        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("OpenRouter template");
        let error = core
            .begin_provider_discovery_known_with_credential_authority(
                SanitizedDiscoveryInput {
                    connection_id: connection_id.clone(),
                    display_name: "Rejected cached discovery".to_owned(),
                    site_url: HttpUrl::parse("https://openrouter.ai/")
                        .expect("OpenRouter site URL"),
                    docs_url: None,
                    credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
                    preferred_assistant: None,
                    connection_options: ProviderDiscoveryConnectionOptions::default(),
                    supplied_evidence_ids: Vec::new(),
                },
                template.id,
                Some(cached_authority),
            )
            .expect_err("terminal removal must invalidate cached discovery authority");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert!(
            core.list_provider_discoveries(10)
                .expect("list rejected discovery work")
                .is_empty(),
            "rejected admission must execute no provider discovery work"
        );
        assert!(
            core.poll_provider_discovery_events(10, Utc::now())
                .expect("poll rejected discovery outbox")
                .is_empty(),
            "rejected admission must publish no provider discovery event"
        );
    }

    #[test]
    fn approved_probe_route_preflight_preserves_exact_route_and_rejects_scope_drift() {
        let first = probe_route("route-a", "/deployments/a/chat/completions");
        let second = probe_route("route-b", "/deployments/b/chat/completions");
        let mut draft = DiscoveryWorkingDraft::new(DiscoverySourceIntent::Site);
        draft.routes = vec![first.clone(), second.clone()];
        draft.probe_route_ids = vec![first.id.clone(), second.id.clone()];
        let budget = standard_probe_budget(2).expect("standard budget");

        let approved = approved_probe_routes(&draft, budget).expect("approved routes");
        assert_eq!(approved, vec![first, second]);
        assert_eq!(
            approved[0]
                .route_config
                .endpoint_path
                .as_ref()
                .map(EndpointPath::as_str),
            Some("/deployments/a/chat/completions")
        );
        assert_eq!(
            approved[1]
                .route_config
                .endpoint_path
                .as_ref()
                .map(EndpointPath::as_str),
            Some("/deployments/b/chat/completions")
        );

        let mut duplicate = draft.clone();
        duplicate.probe_route_ids =
            vec![ModelRouteId::from("route-a"), ModelRouteId::from("route-a")];
        assert!(approved_probe_routes(&duplicate, budget).is_err());

        let mut outside_graph = draft.clone();
        outside_graph.probe_route_ids = vec![
            ModelRouteId::from("route-a"),
            ModelRouteId::from("route-outside"),
        ];
        assert!(approved_probe_routes(&outside_graph, budget).is_err());

        let one_route_budget = standard_probe_budget(1).expect("one-route budget");
        assert!(approved_probe_routes(&draft, one_route_budget).is_err());
    }

    fn exact_openrouter_listed_model() -> lorepia_providers::ListedModel {
        lorepia_providers::ListedModel {
            model_id: "openai/exact-persisted-model".to_owned(),
            display_name: Some("Exact persisted model".to_owned()),
            max_input_tokens: Some(128_000),
            max_output_tokens: Some(16_384),
            supported_generation_methods: Vec::new(),
            capabilities: lorepia_providers::ListedModelCapabilities {
                supported: vec![
                    lorepia_providers::ListedModelCapability::Reasoning,
                    lorepia_providers::ListedModelCapability::ToolCalling,
                    lorepia_providers::ListedModelCapability::ParallelToolCalling,
                    lorepia_providers::ListedModelCapability::StructuredOutput,
                    lorepia_providers::ListedModelCapability::JsonMode,
                    lorepia_providers::ListedModelCapability::Logprobs,
                    lorepia_providers::ListedModelCapability::Seed,
                ],
                parameters: lorepia_providers::OpenRouterSupportedParameterSupport::Exact(vec![
                    lorepia_providers::OpenRouterSupportedParameter::Logprobs,
                    lorepia_providers::OpenRouterSupportedParameter::MaxCompletionTokens,
                    lorepia_providers::OpenRouterSupportedParameter::MaxTokens,
                    lorepia_providers::OpenRouterSupportedParameter::ParallelToolCalls,
                    lorepia_providers::OpenRouterSupportedParameter::Reasoning,
                    lorepia_providers::OpenRouterSupportedParameter::ResponseFormat,
                    lorepia_providers::OpenRouterSupportedParameter::Seed,
                    lorepia_providers::OpenRouterSupportedParameter::StructuredOutputs,
                    lorepia_providers::OpenRouterSupportedParameter::Temperature,
                    lorepia_providers::OpenRouterSupportedParameter::Tools,
                    lorepia_providers::OpenRouterSupportedParameter::TopP,
                ]),
                reasoning: Some(lorepia_providers::ListedModelReasoningCapability {
                    supported_efforts: lorepia_providers::OpenRouterReasoningEffortSupport::Exact(
                        vec![
                            lorepia_providers::OpenRouterReasoningEffort::High,
                            lorepia_providers::OpenRouterReasoningEffort::Low,
                        ],
                    ),
                    default_effort: Some(lorepia_providers::OpenRouterReasoningEffort::High),
                    default_enabled: Some(true),
                    supports_max_tokens: Some(true),
                    mandatory: Some(false),
                }),
            },
            source: lorepia_providers::ModelRecordSource::ProviderApi,
            availability: ModelAvailability::Available,
        }
    }

    fn prepare_openrouter_credential_origin_approval(
        core: &crate::Core,
        connection_id: &str,
    ) -> DiscoverySessionSnapshot {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("OpenRouter template");
        let connection_id = ProviderConnectionId::from(connection_id);
        let selecting = core
            .begin_provider_discovery_known(
                SanitizedDiscoveryInput {
                    connection_id: connection_id.clone(),
                    display_name: "Pre-commit credential provider".to_owned(),
                    site_url: HttpUrl::parse("https://openrouter.ai/")
                        .expect("OpenRouter site URL"),
                    docs_url: None,
                    credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
                    preferred_assistant: None,
                    connection_options: ProviderDiscoveryConnectionOptions::default(),
                    supplied_evidence_ids: Vec::new(),
                },
                template.id.clone(),
            )
            .expect("begin provider discovery");
        let candidate = core
            .list_provider_discovery_candidates(&selecting.session.id)
            .expect("list template candidates")
            .into_iter()
            .find(|candidate| {
                matches!(
                    &candidate.candidate.summary,
                    DiscoveryCandidateSummary::ProviderTemplate {
                        template_id,
                        template_version,
                    } if template_id == &template.id
                        && *template_version == template.manifest_version
                )
            })
            .expect("exact OpenRouter template candidate");
        let selected = core
            .continue_provider_discovery(
                &selecting.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    selecting.session.revision,
                    ProviderDiscoveryAction::SelectTemplate {
                        candidate_id: candidate.candidate.id,
                    },
                )
                .expect("select-template action"),
                None,
            )
            .expect("select OpenRouter template");
        assert_eq!(
            selected.session.state,
            DiscoveryState::AwaitingCredentialOriginApproval
        );
        selected
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn cancellation_during_authenticated_probe_prevents_every_later_dispatch() {
        let (origin, request_receiver, release_sender, later_dispatch_receiver, server) =
            spawn_stalling_probe_provider();
        let api_origin = CanonicalOrigin::parse(&origin).expect("canonical probe origin");
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
            .expect("OpenAI-compatible template");
        let auth = template.default_manifest.auth.clone();
        let connection_id = ProviderConnectionId::from("cancelled-authenticated-probes");
        let connection = ProviderConnection {
            id: connection_id.clone(),
            template_id: template.id.clone(),
            template_version: template.manifest_version,
            display_name: "Cancelled authenticated probes".to_owned(),
            api_origin: api_origin.clone(),
            config: ConnectionConfig {
                api_base_path: Some(EndpointPath::parse("/v1").expect("probe base path")),
                network_mode: ProviderNetworkMode::LocalLoopback,
                local_network_approval: None,
                values: vec![lorepia_domain::ConnectionConfigEntry {
                    key: "api_base_url".to_owned(),
                    value: ConnectionConfigValue::Text(format!("{origin}/v1")),
                }],
            },
            credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
            credential_scope: Some(CredentialScope {
                allowed_origins: vec![api_origin],
                auth_binding: auth,
                redirect_policy: CredentialRedirectPolicy::Deny,
            }),
            timeout_seconds: 5,
            status: ConnectionStatus::Untested,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let route_id = ModelRouteId::from("cancelled-authenticated-probe-route");
        let route = ModelRoute {
            id: route_id.clone(),
            connection_id: connection_id.clone(),
            api_family: template.api_family,
            model_id: "cancelled-probe-model".to_owned(),
            display_name: None,
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::Legacy,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: Utc::now(),
            last_seen_at: None,
        };
        let options = ProviderDiscoveryConnectionOptions {
            network_mode: ProviderNetworkMode::LocalLoopback,
            ..ProviderDiscoveryConnectionOptions::default()
        };
        let session = ProviderDiscoverySession::new(
            DiscoverySessionId::from("cancelled-authenticated-probe-session"),
            SanitizedDiscoveryInput {
                connection_id: connection_id.clone(),
                display_name: "Cancelled authenticated probes".to_owned(),
                site_url: HttpUrl::parse(&format!("{origin}/")).expect("probe site URL"),
                docs_url: None,
                credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
                preferred_assistant: None,
                connection_options: options,
                supplied_evidence_ids: Vec::new(),
            },
        )
        .expect("probe session");
        let now = Utc::now();
        let snapshot = DiscoverySessionSnapshot {
            session,
            active_operation_id: None,
            draft_json: None,
            review: None,
            created_at: now,
            updated_at: now,
        };
        let mut draft = DiscoveryWorkingDraft::new(DiscoverySourceIntent::KnownProvider {
            template_id: template.id.clone(),
        });
        draft.template = Some(template);
        draft.connection = Some(connection);
        draft.routes = vec![route];
        draft.probe_route_ids = vec![route_id];
        let budget = standard_probe_budget(1).expect("standard probe budget");
        let (cancel_sender, cancelled) = watch::channel(false);
        let runtime = Arc::new(tokio::runtime::Runtime::new().expect("probe runtime"));
        let worker_runtime = Arc::clone(&runtime);
        let worker = thread::spawn(move || {
            probe_draft(
                worker_runtime.handle(),
                &snapshot,
                &mut draft,
                Some("authenticated-probe-secret"),
                budget,
                cancelled,
            )
        });

        let request = request_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("first authenticated probe dispatched");
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer authenticated-probe-secret\r\n")
        );
        cancel_sender
            .send(true)
            .expect("cancel in-flight authenticated probe");
        thread::sleep(Duration::from_millis(50));
        release_sender.send(()).expect("release first probe socket");

        let outcome = worker
            .join()
            .expect("join probe worker")
            .expect("probe cancellation outcome");
        assert!(matches!(outcome, ProbeExecution::Unknown));
        assert!(
            !later_dispatch_receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("later dispatch observation"),
            "no later authenticated probe may dispatch after cancellation"
        );
        server.join().expect("join probe provider");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn request_cancellation_does_not_fake_completion_of_started_authenticated_listing() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
        let selected =
            prepare_openrouter_credential_origin_approval(&core, "started-list-cancellation");
        let proposal = core
            .get_provider_discovery_approval_proposal(&selected.session.id)
            .expect("load credential-origin proposal")
            .expect("credential-origin proposal");
        let orchestrator = core.provider_discovery();
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            selected.session.revision,
            ProviderDiscoveryAction::ApproveCredentialOrigin {
                approval_id: proposal.id,
            },
        )
        .expect("approve credential origin");
        let mut draft = hydrate_working_draft(&selected).expect("hydrate selected draft");
        let occurred_at = Utc::now();
        let (approval, review, prepared_commit) = orchestrator
            .prepare_user_action(&selected, &envelope, &mut draft, occurred_at)
            .expect("prepare credential approval");
        let transition = selected
            .session
            .apply(&envelope)
            .expect("apply credential approval");
        let operation_id = DiscoveryOperationId::new();
        orchestrator
            .storage
            .persist_discovery_transition(&DiscoveryTransitionWrite {
                transition,
                draft: DiscoveryJsonUpdate::Replace(
                    working_draft_value(&draft).expect("serialize approved draft"),
                ),
                review,
                new_evidence: Vec::new(),
                new_candidates: Vec::new(),
                approval,
                new_operation_id: Some(operation_id.clone()),
                completed_operation: None,
                prepared_commit,
                provider_graph: None,
                occurred_at,
            })
            .expect("persist listing operation");
        assert!(
            orchestrator
                .storage
                .mark_discovery_operation_started(&operation_id, Utc::now())
                .expect("start authenticated listing")
        );
        let listing = core
            .get_provider_discovery(&selected.session.id)
            .expect("load started listing");

        let cancelling = core
            .cancel_provider_discovery(&listing.session.id, listing.session.revision)
            .expect("persist cancellation request");

        assert_eq!(cancelling.session.state, DiscoveryState::ListingModels);
        assert!(cancelling.session.cancellation_pending);
        let active = core
            .storage()
            .get_current_discovery_operation(&listing.session.id)
            .expect("load active listing")
            .expect("started listing remains active");
        assert_eq!(active.id, operation_id);
        assert_eq!(active.status, DiscoveryOperationStatus::Started);

        let rebased = orchestrator
            .inflight_completion_snapshot(&listing, &operation_id)
            .expect("rebase worker completion onto cancellation revision");
        assert_eq!(rebased.session.revision, cancelling.session.revision);
        let mut worker_draft =
            hydrate_working_draft(&listing).expect("hydrate in-flight worker draft");
        orchestrator
            .persist_operation_completion(
                &rebased,
                &operation_id,
                &mut worker_draft,
                ProviderDiscoveryAction::Interrupt {
                    operation: DiscoveryOperationKind::ListModels,
                    outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                },
                DurableOperationOutcome::Interrupted,
                Vec::new(),
                Vec::new(),
                DiscoveryJsonUpdate::Preserve,
            )
            .expect("settle actual cancelled worker outcome");
        let settled = core
            .get_provider_discovery(&listing.session.id)
            .expect("load settled cancellation");
        assert_eq!(settled.session.state, DiscoveryState::Cancelled);
        assert!(!settled.session.cancellation_pending);
    }

    #[test]
    fn discovery_credential_lease_is_stable_from_origin_approval_through_review() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
        let selected =
            prepare_openrouter_credential_origin_approval(&core, "precommit-lease-stable");
        let proposal = core
            .get_provider_discovery_approval_proposal(&selected.session.id)
            .expect("load credential-origin proposal")
            .expect("credential-origin proposal");
        let before_approval = core
            .get_provider_discovery_credential_lease_context(&selected.session.id)
            .expect("prospective credential lease context");
        assert_eq!(before_approval.session_id, selected.session.id);
        assert_eq!(
            before_approval.connection_id.as_str(),
            "precommit-lease-stable"
        );
        assert_eq!(before_approval.credential_origin_approval_id, proposal.id);
        assert_eq!(
            before_approval.credential_origin_grant_sha256,
            proposal.grant_sha256
        );
        assert_eq!(before_approval.connection_binding_sha256.len(), 64);

        let listed = approve_credential_and_seed_model_listing(
            &core,
            &selected,
            proposal.id,
            &[exact_openrouter_listed_model()],
        );
        assert_eq!(listed.session.state, DiscoveryState::AwaitingProbeConsent);
        assert_eq!(
            core.get_provider_discovery_credential_lease_context(&listed.session.id)
                .expect("post-listing credential lease context"),
            before_approval
        );

        let reviewed = core
            .continue_provider_discovery(
                &listed.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    listed.session.revision,
                    ProviderDiscoveryAction::SkipProbes,
                )
                .expect("skip-probes action"),
                None,
            )
            .expect("skip capability probes");
        assert_eq!(reviewed.session.state, DiscoveryState::AwaitingReview);
        assert_eq!(
            core.get_provider_discovery_credential_lease_context(&reviewed.session.id)
                .expect("review credential lease context"),
            before_approval
        );

        drop(core);
        let core = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
        assert_eq!(
            core.get_provider_discovery_credential_lease_context(&reviewed.session.id)
                .expect("reopened review credential lease context"),
            before_approval
        );

        let cancelled = core
            .cancel_provider_discovery(&reviewed.session.id, reviewed.session.revision)
            .expect("cancel pre-commit discovery");
        assert_eq!(cancelled.session.state, DiscoveryState::Cancelled);
        assert!(
            core.get_provider_discovery_credential_lease_context(&cancelled.session.id)
                .is_err(),
            "terminal discovery must not retain credential lease authority"
        );
    }

    #[test]
    fn discovery_credential_lease_survives_only_list_or_probe_interruption() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");

        let selected =
            prepare_openrouter_credential_origin_approval(&core, "precommit-list-interrupted");
        let proposal = core
            .get_provider_discovery_approval_proposal(&selected.session.id)
            .expect("load credential-origin proposal")
            .expect("credential-origin proposal");
        let expected = core
            .get_provider_discovery_credential_lease_context(&selected.session.id)
            .expect("prospective credential lease context");
        let interrupted = core
            .continue_provider_discovery(
                &selected.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    selected.session.revision,
                    ProviderDiscoveryAction::ApproveCredentialOrigin {
                        approval_id: proposal.id,
                    },
                )
                .expect("approve credential-origin action"),
                None,
            )
            .expect("interrupt credential-bound model listing without a credential");
        assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
        assert_eq!(
            interrupted
                .session
                .recovery
                .as_ref()
                .map(|recovery| recovery.operation),
            Some(DiscoveryOperationKind::ListModels)
        );
        assert_eq!(
            core.get_provider_discovery_credential_lease_context(&interrupted.session.id)
                .expect("interrupted listing credential lease context"),
            expected
        );

        let selected =
            prepare_openrouter_credential_origin_approval(&core, "precommit-probe-interrupted");
        let proposal = core
            .get_provider_discovery_approval_proposal(&selected.session.id)
            .expect("load second credential-origin proposal")
            .expect("second credential-origin proposal");
        let expected = core
            .get_provider_discovery_credential_lease_context(&selected.session.id)
            .expect("second prospective credential lease context");
        let listed = approve_credential_and_seed_model_listing(
            &core,
            &selected,
            proposal.id,
            &[exact_openrouter_listed_model()],
        );
        let probe = core
            .get_provider_discovery_approval_proposal(&listed.session.id)
            .expect("load probe proposal")
            .expect("probe proposal");
        let interrupted = core
            .continue_provider_discovery(
                &listed.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    listed.session.revision,
                    ProviderDiscoveryAction::ApproveProbes {
                        approval_id: probe.id,
                        approval_grant_sha256: probe.grant_sha256,
                    },
                )
                .expect("approve probes action"),
                None,
            )
            .expect("interrupt credential-bound probes without a credential");
        assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
        assert_eq!(
            interrupted
                .session
                .recovery
                .as_ref()
                .map(|recovery| recovery.operation),
            Some(DiscoveryOperationKind::ProbeCapabilities)
        );
        assert_eq!(
            core.get_provider_discovery_credential_lease_context(&interrupted.session.id)
                .expect("interrupted probe credential lease context"),
            expected
        );
    }

    #[test]
    fn discovery_credential_lease_rejects_origin_auth_and_connection_binding_drift() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
        let selected =
            prepare_openrouter_credential_origin_approval(&core, "precommit-lease-drift");
        let proposal = core
            .get_provider_discovery_approval_proposal(&selected.session.id)
            .expect("load credential-origin proposal")
            .expect("credential-origin proposal");
        let listed = approve_credential_and_seed_model_listing(
            &core,
            &selected,
            proposal.id,
            &[exact_openrouter_listed_model()],
        );
        let approval = core
            .list_provider_discovery_approvals(&listed.session.id)
            .expect("list discovery approvals")
            .into_iter()
            .find(|approval| {
                matches!(
                    &approval.grant,
                    DiscoveryApprovalGrant::CredentialOrigin { .. }
                )
            })
            .expect("durable credential-origin approval");
        let draft = hydrate_working_draft(&listed).expect("hydrate approved draft");
        validate_credential_origin_approval(&listed, &draft, &approval)
            .expect("unchanged approval binding");
        let connection = draft.connection.as_ref().expect("approved connection");
        validated_discovery_credential_binding_sha256(&listed, &draft, connection)
            .expect("unchanged final binding");

        let mut origin_drift = draft.clone();
        origin_drift
            .connection
            .as_mut()
            .expect("connection")
            .api_origin = CanonicalOrigin::parse("https://drift.example").expect("drift origin");
        assert!(validate_credential_origin_approval(&listed, &origin_drift, &approval).is_err());

        let mut auth_drift = draft.clone();
        auth_drift
            .connection
            .as_mut()
            .expect("connection")
            .credential_scope
            .as_mut()
            .expect("credential scope")
            .auth_binding = AuthBinding::None;
        assert!(validate_credential_origin_approval(&listed, &auth_drift, &approval).is_err());

        let mut binding_drift = draft.clone();
        binding_drift
            .connection
            .as_mut()
            .expect("connection")
            .config
            .api_base_path = Some(EndpointPath::parse("/drift").expect("drift base path"));
        assert!(
            validated_discovery_credential_binding_sha256(
                &listed,
                &binding_drift,
                binding_drift.connection.as_ref().expect("connection"),
            )
            .is_err()
        );
    }

    fn approve_credential_and_seed_model_listing(
        core: &crate::Core,
        snapshot: &DiscoverySessionSnapshot,
        approval_id: DiscoveryApprovalId,
        listed_models: &[lorepia_providers::ListedModel],
    ) -> DiscoverySessionSnapshot {
        let orchestrator = core.provider_discovery();
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::ApproveCredentialOrigin { approval_id },
        )
        .expect("approve-credential action");
        let mut draft = hydrate_working_draft(snapshot).expect("hydrate credential draft");
        let occurred_at = Utc::now();
        let (approval, review, prepared_commit) = orchestrator
            .prepare_user_action(snapshot, &envelope, &mut draft, occurred_at)
            .expect("prepare credential approval");
        let transition = snapshot
            .session
            .apply(&envelope)
            .expect("apply credential approval");
        let new_operation_id =
            operation_for_effect(&transition.effect).map(|_| DiscoveryOperationId::new());
        orchestrator
            .storage
            .persist_discovery_transition(&DiscoveryTransitionWrite {
                transition,
                draft: DiscoveryJsonUpdate::Replace(
                    working_draft_value(&draft).expect("serialize credential-approved draft"),
                ),
                review,
                new_evidence: Vec::new(),
                new_candidates: Vec::new(),
                approval,
                new_operation_id,
                completed_operation: None,
                prepared_commit,
                provider_graph: None,
                occurred_at,
            })
            .expect("persist credential approval without running network");

        let listing = orchestrator
            .get(&snapshot.session.id)
            .expect("load model-list operation");
        assert_eq!(listing.session.state, DiscoveryState::ListingModels);
        let operation = orchestrator
            .storage
            .get_current_discovery_operation(&snapshot.session.id)
            .expect("load current model-list operation")
            .expect("model-list operation");
        assert_eq!(operation.kind, DiscoveryOperationKind::ListModels);
        assert!(
            orchestrator
                .storage
                .mark_discovery_operation_started(&operation.id, Utc::now())
                .expect("start model-list operation"),
            "prepared model-list operation must start exactly once"
        );
        let mut draft = hydrate_working_draft(&listing).expect("hydrate model-list draft");
        apply_listed_models_to_draft(&mut draft, listed_models, Utc::now())
            .expect("apply canonical normalized OpenRouter listing");
        draft.probe_route_ids = draft.routes.iter().map(|route| route.id.clone()).collect();
        let model_count = u32::try_from(draft.routes.len()).expect("bounded model count");
        let candidates = model_candidates(&listing, &draft).expect("build model candidates");
        orchestrator
            .persist_operation_completion(
                &listing,
                &operation.id,
                &mut draft,
                ProviderDiscoveryAction::ModelsListed {
                    model_count,
                    probe_candidate_count: model_count,
                },
                DurableOperationOutcome::Succeeded,
                Vec::new(),
                candidates,
                DiscoveryJsonUpdate::Preserve,
            )
            .expect("persist normalized model-list completion");
        orchestrator
            .get(&snapshot.session.id)
            .expect("load seeded model-list result")
    }

    fn prepare_no_network_credential_commit(
        core: &crate::Core,
        connection_id: &str,
    ) -> DiscoverySessionSnapshot {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("OpenRouter template");
        let connection_id = ProviderConnectionId::from(connection_id);
        let selecting = core
            .begin_provider_discovery_known(
                SanitizedDiscoveryInput {
                    connection_id: connection_id.clone(),
                    display_name: "No-network recovery provider".to_owned(),
                    site_url: HttpUrl::parse("https://openrouter.ai/")
                        .expect("OpenRouter site URL"),
                    docs_url: None,
                    credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
                    preferred_assistant: None,
                    connection_options: ProviderDiscoveryConnectionOptions::default(),
                    supplied_evidence_ids: Vec::new(),
                },
                template.id.clone(),
            )
            .expect("begin no-network provider discovery");
        finish_no_network_credential_commit(core, &template, &selecting)
    }

    fn finish_no_network_credential_commit(
        core: &crate::Core,
        template: &ProviderTemplate,
        selecting: &DiscoverySessionSnapshot,
    ) -> DiscoverySessionSnapshot {
        let candidate = core
            .list_provider_discovery_candidates(&selecting.session.id)
            .expect("list template candidates")
            .into_iter()
            .find(|candidate| {
                matches!(
                    &candidate.candidate.summary,
                    DiscoveryCandidateSummary::ProviderTemplate {
                        template_id,
                        template_version,
                    } if template_id == &template.id
                        && *template_version == template.manifest_version
                )
            })
            .expect("exact OpenRouter template candidate");
        let selected = core
            .continue_provider_discovery(
                &selecting.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    selecting.session.revision,
                    ProviderDiscoveryAction::SelectTemplate {
                        candidate_id: candidate.candidate.id,
                    },
                )
                .expect("select-template action"),
                None,
            )
            .expect("select no-network provider template");
        let credential_proposal = core
            .get_provider_discovery_approval_proposal(&selected.session.id)
            .expect("load credential-origin proposal")
            .expect("credential-origin proposal");
        let listed = approve_credential_and_seed_model_listing(
            core,
            &selected,
            credential_proposal.id,
            &[exact_openrouter_listed_model()],
        );
        let reviewed = core
            .continue_provider_discovery(
                &listed.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    listed.session.revision,
                    ProviderDiscoveryAction::SkipProbes,
                )
                .expect("skip-probes action"),
                None,
            )
            .expect("skip no-network capability probes");
        let proposal = core
            .get_provider_discovery_review_proposal(&reviewed.session.id)
            .expect("load review proposal")
            .expect("review proposal");
        let committing = core
            .continue_provider_discovery(
                &reviewed.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    reviewed.session.revision,
                    ProviderDiscoveryAction::ApproveReview {
                        approval_id: proposal.approval.id,
                        commit_attempt_id: proposal.commit_attempt_id,
                        commit_plan_sha256: proposal.commit_plan_sha256,
                        graph_sha256: proposal.review.graph_sha256,
                    },
                )
                .expect("approve-review action"),
                None,
            )
            .expect("prepare no-network credential commit");
        assert_eq!(committing.session.state, DiscoveryState::Committing);
        committing
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Keeps the real lock/expiry timeline visible in one fixture.
    fn credential_graph_publication_rechecks_lan_expiry_after_sqlite_write_lock_wait() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
            .expect("custom OpenAI-compatible template");
        let connection_id = ProviderConnectionId::from("lan-lock-expiry-publication");
        let selecting = core
            .begin_provider_discovery_known(
                SanitizedDiscoveryInput {
                    connection_id: connection_id.clone(),
                    display_name: "LAN lock expiry provider".to_owned(),
                    site_url: HttpUrl::parse("https://models.lan:8443/")
                        .expect("approved LAN site URL"),
                    docs_url: None,
                    credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
                    preferred_assistant: None,
                    connection_options: ProviderDiscoveryConnectionOptions {
                        network_mode: ProviderNetworkMode::ApprovedLocalNetwork,
                        local_network_approval: Some(ProviderLocalNetworkApproval {
                            origin: CanonicalOrigin::parse("https://models.lan:8443")
                                .expect("approved credential-bearing LAN origin"),
                            addresses: vec!["192.168.10.20".parse::<IpAddr>().unwrap()],
                        }),
                        local_network_approved_at: Some(Utc::now()),
                        ..ProviderDiscoveryConnectionOptions::default()
                    },
                    supplied_evidence_ids: Vec::new(),
                },
                template.id.clone(),
            )
            .expect("begin LAN provider discovery");

        // Preparing the credential-backed graph can contend with other long-running
        // Core tests. Keep a wide setup margin while still crossing a real expiry
        // boundary under the SQLite write lock below.
        let expires_at = Utc::now() + chrono::Duration::seconds(60);
        let approved_at = expires_at - chrono::Duration::hours(24);
        let mut aged_input = selecting.session.input.clone();
        aged_input.connection_options.local_network_approved_at = Some(approved_at);
        let mut input_json = String::new();
        write_canonical_json(
            &serde_json::to_value(&aged_input).expect("serialize aged LAN input"),
            &mut input_json,
        )
        .expect("canonicalize aged LAN input");
        let database_path = active_test_database_path(root.path());
        let fixture =
            rusqlite::Connection::open(&database_path).expect("open LAN fixture database");
        let revision_guard = fixture
            .query_row(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'trigger' AND name = 'provider_discovery_session_revision_guard'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("load discovery revision guard");
        fixture
            .execute_batch("DROP TRIGGER provider_discovery_session_revision_guard")
            .expect("suspend revision guard for immutable-time fixture");
        assert_eq!(
            fixture
                .execute(
                    "UPDATE provider_discovery_sessions
                     SET sanitized_input_json = ?2, created_at = ?3
                     WHERE id = ?1",
                    rusqlite::params![
                        selecting.session.id.as_str(),
                        input_json,
                        approved_at.to_rfc3339(),
                    ],
                )
                .expect("age LAN session authority"),
            1
        );
        fixture
            .execute_batch(&revision_guard)
            .expect("restore discovery revision guard");
        drop(fixture);

        let committing = finish_no_network_credential_commit(&core, &template, &selecting);
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("prepared credential install context");
        let started = reserve_and_start_credential_install(&core, &prepared);
        let confirmation = credential_commit_confirmation(&started);
        let lock_at = expires_at - chrono::Duration::seconds(5);
        assert!(
            Utc::now() < lock_at,
            "fixture must finish before the bounded SQLite lock-wait window"
        );
        while Utc::now() < lock_at {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let blocker =
            rusqlite::Connection::open(&database_path).expect("open SQLite write-lock blocker");
        blocker
            .execute_batch("BEGIN IMMEDIATE")
            .expect("acquire SQLite write lock before LAN expiry");
        let error = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                core.commit_provider_discovery(&committing.session.id, Some(&confirmation))
            });
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert!(
                !worker.is_finished(),
                "publication must be waiting on the real SQLite write lock"
            );
            while Utc::now() < expires_at {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            blocker
                .execute_batch("COMMIT")
                .expect("release SQLite write lock after LAN expiry");
            worker
                .join()
                .expect("publication worker")
                .expect_err("expired LAN authority must not publish a provider graph")
        });
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert!(
            core.storage()
                .get_provider_connection(&connection_id)
                .is_err(),
            "expired authority must leave the provider graph unpublished"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn credential_bound_commit_rejects_prepared_wal_until_native_install_is_started() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
        let committing =
            prepare_no_network_credential_commit(&core, "credential-commit-start-required");
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("prepared credential install context");
        assert_eq!(
            prepared.operation_status,
            DiscoveryOperationStatus::Prepared
        );
        assert_eq!(prepared.native_execution_reservation_id, None);
        assert_eq!(prepared.native_execution_id, None);

        assert!(
            ProviderDiscoveryCredentialCommitConfirmation::try_from(&prepared).is_err(),
            "a prepared operation has no physical native authority to confirm"
        );
        let prepared_confirmation = ProviderDiscoveryCredentialCommitConfirmation {
            operation_id: prepared.operation_id.clone(),
            native_execution_id: "rolled-back-native-execution-A".to_owned(),
            commit_attempt_id: prepared.commit_attempt_id.clone(),
            commit_plan_sha256: prepared.commit_plan_sha256.clone(),
            connection_id: prepared.connection_id.clone(),
            connection_binding_sha256: prepared.connection_binding_sha256.clone(),
        };
        let error = core
            .commit_provider_discovery(&committing.session.id, Some(&prepared_confirmation))
            .expect_err("a future exact envelope must not adopt a rolled-back prepared WAL");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        let unchanged = core
            .get_provider_discovery(&committing.session.id)
            .expect("reload rejected prepared commit");
        assert_eq!(unchanged.session.state, DiscoveryState::Committing);
        assert_eq!(
            core.get_provider_discovery_credential_install_context(&committing.session.id)
                .expect("reload prepared credential install context")
                .operation_status,
            DiscoveryOperationStatus::Prepared
        );
        assert!(
            core.list_provider_connections()
                .expect("list provider connections")
                .iter()
                .all(|connection| connection.id != prepared.connection_id),
            "rejected prepared WAL must publish no provider graph"
        );

        let reserved = reserve_credential_install(&core, &prepared);
        let exact_reservation_id = reserved
            .native_execution_reservation_id
            .as_deref()
            .expect("exact reserved physical authority");
        let stale_operation = core
            .start_provider_discovery_credential_install(
                &reserved.session_id,
                reserved.session_revision,
                &DiscoveryOperationId::new(),
                &reserved.commit_attempt_id,
                &reserved.commit_plan_sha256,
                exact_reservation_id,
            )
            .expect_err("stale semantic provenance must fail before consuming exact B");
        assert_eq!(stale_operation.code, CoreErrorCode::InvalidInput);
        let forged = core
            .start_provider_discovery_credential_install(
                &reserved.session_id,
                reserved.session_revision,
                &reserved.operation_id,
                &reserved.commit_attempt_id,
                &reserved.commit_plan_sha256,
                "discovery-native-00000000-0000-4000-8000-000000000000",
            )
            .expect_err("an unregistered physical reservation cannot start a store");
        assert_eq!(forged.code, CoreErrorCode::InvalidInput);
        let cloned_core = core.clone();
        let started = start_reserved_credential_install(&cloned_core, &reserved);
        assert!(started.native_execution_id.is_some());
        let mut legacy_unbound_started = started.clone();
        legacy_unbound_started.native_execution_reservation_id = None;
        legacy_unbound_started.native_execution_id = None;
        assert!(
            ProviderDiscoveryCredentialCommitConfirmation::try_from(&legacy_unbound_started)
                .is_err(),
            "a migrated Started lineage without physical authority is recovery-only"
        );
        let confirmation = credential_commit_confirmation(&started);
        let mut stale_physical_confirmation = confirmation.clone();
        stale_physical_confirmation.native_execution_id =
            "rolled-back-native-execution-A".to_owned();
        let error = core
            .commit_provider_discovery(&committing.session.id, Some(&stale_physical_confirmation))
            .expect_err("semantic commit provenance must not adopt another physical incarnation");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            core.get_provider_discovery_credential_install_context(&committing.session.id)
                .expect("reload started context after stale physical confirmation"),
            started
        );
        let committed = core
            .commit_provider_discovery(&committing.session.id, Some(&confirmation))
            .expect("commit explicitly-started credential installation");
        assert_eq!(committed.id, prepared.connection_id);
        assert!(
            core.list_provider_connections()
                .expect("list committed provider connections")
                .iter()
                .any(|connection| connection.id == committed.id)
        );
    }

    #[test]
    fn legacy_unbound_started_execution_is_exposed_only_for_conservative_recovery() {
        let started_at = Utc::now();
        assert!(
            native_credential_execution_context_ids(
                DiscoveryOperationStatus::Started,
                Some(&started_at),
                None,
                false,
            )
            .is_err(),
            "normal install context must reject a Started lineage without physical authority"
        );
        assert_eq!(
            native_credential_execution_context_ids(
                DiscoveryOperationStatus::Started,
                Some(&started_at),
                None,
                true,
            )
            .expect("sealed legacy Started lineage is readable only for recovery"),
            (None, None)
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn schema36_started_cancel_crash_upgrades_without_synthesizing_physical_authority() {
        let schema36_root = tempdir().expect("temporary schema-36 source root");
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(schema36_root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open current Core before exact schema inverse");
        let committing =
            prepare_no_network_credential_commit(&core, "schema36-started-cancel-cutoff");
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("load schema-36 fixture commit operation");
        let schema36_database = active_test_database_path(schema36_root.path());
        reverse_schema37_credential_migration(&schema36_database);
        assert!(
            core.storage()
                .mark_discovery_operation_started(&prepared.operation_id, Utc::now())
                .expect("start exact credential operation under schema 36")
        );
        let schema36_snapshot = core
            .get_provider_discovery(&committing.session.id)
            .expect("load schema-36 Started discovery");
        let cancelling = core
            .provider_discovery()
            .continue_discovery(
                &committing.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    schema36_snapshot.session.revision,
                    ProviderDiscoveryAction::Cancel,
                )
                .expect("build schema-36 cancellation action"),
                None,
            )
            .expect("persist schema-36 Started cancellation");
        assert!(cancelling.session.cancellation_pending);
        assert!(cancelling.session.revision > prepared.session_revision);
        drop(core);
        checkpoint_test_database(&schema36_database);

        let upgraded_root = tempdir().expect("temporary schema-37 upgrade root");
        let canonical_database = upgraded_root.path().join("db/lorepia.sqlite3");
        std::fs::create_dir_all(
            canonical_database
                .parent()
                .expect("canonical database parent"),
        )
        .expect("create canonical database directory");
        std::fs::copy(&schema36_database, &canonical_database)
            .expect("copy genuine schema-36 fixture into upgrade root");

        let upgraded = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(upgraded_root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("upgrade genuine schema-36 Started cancellation");
        assert_eq!(
            upgraded.storage().schema_version().expect("schema version"),
            40
        );
        upgraded
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect_err("legacy unbound Started lineage is never normal install authority");
        let recovery = upgraded
            .get_provider_discovery_credential_install_recovery_context(&committing.session.id)
            .expect("load sealed legacy Started recovery context");
        assert_eq!(recovery.operation_status, DiscoveryOperationStatus::Started);
        assert_eq!(recovery.operation_id, prepared.operation_id);
        assert_eq!(recovery.native_execution_reservation_id, None);
        assert_eq!(recovery.native_execution_id, None);
        assert!(
            ProviderDiscoveryCredentialCommitConfirmation::try_from(&recovery).is_err(),
            "legacy semantic start cannot become physical commit confirmation"
        );

        let upgraded_database = active_test_database_path(upgraded_root.path());
        let connection =
            rusqlite::Connection::open(&upgraded_database).expect("open upgraded database");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*)
                     FROM provider_discovery_native_credential_legacy_started_cutoff_snapshots
                     WHERE operation_id = ?1",
                    [recovery.operation_id.as_str()],
                    |row| row.get::<_, u64>(0),
                )
                .expect("count sealed legacy Started cutoff"),
            1
        );
        for table in [
            "provider_discovery_native_credential_executions",
            "provider_discovery_native_credential_store_attempts",
        ] {
            assert_eq!(
                connection
                    .query_row(
                        &format!("SELECT COUNT(*) FROM {table} WHERE operation_id = ?1"),
                        [recovery.operation_id.as_str()],
                        |row| row.get::<_, u64>(0),
                    )
                    .unwrap_or_else(|error| panic!("count {table}: {error}")),
                0,
                "schema upgrade must not synthesize a physical credential authority"
            );
        }
        drop(connection);

        upgraded
            .recover_provider_discovery(Utc::now())
            .expect("conservatively recover legacy Started lineage");
        let unknown = upgraded
            .get_provider_discovery(&committing.session.id)
            .expect("load legacy outcome-unknown recovery");
        assert_eq!(unknown.session.state, DiscoveryState::UnknownOutcome);
        assert_eq!(
            unknown.session.unknown_operation,
            Some(DiscoveryOperationKind::AtomicCommit)
        );
        let connection =
            rusqlite::Connection::open(&upgraded_database).expect("reopen upgraded database");
        assert_eq!(
            connection
                .query_row(
                    "SELECT action_kind
                     FROM provider_discovery_action_receipts
                     WHERE session_id = ?1 AND resulting_revision = ?2",
                    rusqlite::params![committing.session.id.as_str(), unknown.session.revision],
                    |row| row.get::<_, String>(0),
                )
                .expect("load legacy Started terminal receipt kind"),
            "interrupt"
        );
        drop(connection);

        let resolution =
            lorepia_domain::discovery::DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect;
        let proposal = approval_proposal_for(
            &unknown.session.id,
            unknown.session.revision,
            DiscoveryApprovalGrant::UnknownOutcomeResolution {
                operation: DiscoveryOperationKind::AtomicCommit,
                resolution: resolution.clone(),
            },
        )
        .expect("derive exact legacy no-effect approval");
        let interrupted = upgraded
            .continue_provider_discovery(
                &unknown.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    unknown.session.revision,
                    ProviderDiscoveryAction::ResolveUnknownOutcome {
                        approval_id: proposal.id,
                        resolution,
                    },
                )
                .expect("build legacy no-effect resolution action"),
                None,
            )
            .expect("resolve legacy Started outcome as no effect");
        assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
        assert!(
            upgraded
                .storage()
                .get_discovery_native_credential_execution(&recovery.operation_id)
                .expect("reload historical legacy execution after no-effect resolution")
                .is_none()
        );

        let restarted = upgraded
            .continue_provider_discovery(
                &interrupted.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    interrupted.session.revision,
                    ProviderDiscoveryAction::RestartInterrupted,
                )
                .expect("build explicit legacy restart action"),
                None,
            )
            .expect("restart legacy no-effect recovery");
        assert_eq!(restarted.session.state, DiscoveryState::Compensating);
        assert_ne!(
            restarted
                .active_operation_id
                .as_ref()
                .expect("restarted legacy descendant operation"),
            &recovery.operation_id
        );
        assert!(
            upgraded
                .storage()
                .get_discovery_native_credential_execution(&recovery.operation_id)
                .expect("reload historical legacy execution after descendant")
                .is_none()
        );
        drop(upgraded);

        let reopened = open_core_after_drop(
            upgraded_root.path(),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        );
        assert_eq!(
            reopened
                .get_provider_discovery(&committing.session.id)
                .expect("reload migrated legacy descendant")
                .session
                .state,
            DiscoveryState::Compensating
        );
        assert!(
            reopened
                .storage()
                .get_discovery_native_credential_execution(&recovery.operation_id)
                .expect("reload legacy physical authority projection")
                .is_none(),
            "legacy recovery remains physically unbound after reopen"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn database_rollback_mints_a_new_physical_execution_for_the_same_prepared_operation() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core with native recovery ownership");
        let committing =
            prepare_no_network_credential_commit(&core, "credential-rollback-incarnation");
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("prepared credential install context");
        assert_eq!(
            prepared.operation_status,
            DiscoveryOperationStatus::Prepared
        );
        assert_eq!(prepared.native_execution_reservation_id, None);
        assert_eq!(prepared.native_execution_id, None);
        drop(core);

        let database = active_test_database_path(root.path());
        checkpoint_test_database(&database);
        let prepared_backup = root.path().join("prepared-credential-rollback.sqlite3");
        std::fs::copy(&database, &prepared_backup).expect("snapshot prepared test database");

        let core = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
        let execution_a = reserve_and_start_credential_install(&core, &prepared);
        let confirmation_a = credential_commit_confirmation(&execution_a);
        drop(core);

        restore_test_database(&database, &prepared_backup);
        let rolled_back =
            open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
        let restored_prepared = rolled_back
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("reload restored prepared context");
        assert_eq!(restored_prepared, prepared);

        let execution_b = reserve_and_start_credential_install(&rolled_back, &restored_prepared);
        assert_eq!(execution_b.operation_id, execution_a.operation_id);
        assert_eq!(execution_b.commit_attempt_id, execution_a.commit_attempt_id);
        assert_eq!(
            execution_b.commit_plan_sha256,
            execution_a.commit_plan_sha256
        );
        assert_ne!(
            execution_b.native_execution_id, execution_a.native_execution_id,
            "rolling durable state back to Prepared must not reuse execution A"
        );

        let error = rolled_back
            .commit_provider_discovery(&committing.session.id, Some(&confirmation_a))
            .expect_err("execution A must not confirm the rolled-back execution B");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            rolled_back
                .get_provider_discovery_credential_install_context(&committing.session.id)
                .expect("reload execution B after stale A confirmation"),
            execution_b
        );

        let stale_attestation = rolled_back
            .attest_provider_discovery_credential_install_no_effect(
                &committing.session.id,
                &execution_b.operation_id,
                &execution_b.commit_attempt_id,
                &execution_b.commit_plan_sha256,
                native_execution_id(&execution_a),
            )
            .expect_err("execution A cannot attest the rolled-back execution B missing");
        assert_eq!(stale_attestation.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            rolled_back
                .get_provider_discovery_credential_install_context(&committing.session.id)
                .expect("reload execution B after stale A attestation"),
            execution_b
        );
        let interrupted = rolled_back
            .attest_provider_discovery_credential_install_no_effect(
                &committing.session.id,
                &execution_b.operation_id,
                &execution_b.commit_attempt_id,
                &execution_b.commit_plan_sha256,
                native_execution_id(&execution_b),
            )
            .expect("execution B can attest its own exact slot missing");
        assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
        let attestation = rolled_back
            .storage()
            .get_discovery_native_no_effect_attestation(&execution_b.operation_id)
            .expect("load execution B no-effect evidence")
            .expect("execution B no-effect evidence");
        assert_eq!(
            attestation.physical_authority_id,
            native_execution_id(&execution_b)
        );
        drop(rolled_back);

        let reopened = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
        assert_eq!(
            reopened
                .get_provider_discovery(&committing.session.id)
                .expect("reload execution B interruption")
                .session
                .state,
            DiscoveryState::Interrupted
        );
        assert_eq!(
            reopened
                .storage()
                .get_discovery_native_no_effect_attestation(&execution_b.operation_id)
                .expect("reload execution B no-effect evidence")
                .expect("durable execution B no-effect evidence"),
            attestation
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn reopened_rolled_back_reservation_cannot_reuse_a_lost_store_attempt() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core with native recovery ownership");
        let committing =
            prepare_no_network_credential_commit(&core, "credential-reservation-rollback");
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("prepared credential install context");
        let reserved_b = reserve_credential_install(&core, &prepared);

        let database = active_test_database_path(root.path());
        checkpoint_test_database(&database);
        let reserved_backup = root.path().join("reserved-credential-rollback.sqlite3");
        std::fs::copy(&database, &reserved_backup).expect("snapshot reserved test database");

        let started_b = start_reserved_credential_install(&core, &reserved_b);
        let confirmation_b = credential_commit_confirmation(&started_b);
        drop(core);

        restore_test_database(&database, &reserved_backup);
        let reopened =
            open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
        let rolled_back = reopened
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("load rolled-back reserved context");
        assert_eq!(rolled_back, reserved_b);
        assert_eq!(
            rolled_back.operation_status,
            DiscoveryOperationStatus::Prepared
        );
        assert!(rolled_back.native_execution_reservation_id.is_some());
        assert_eq!(rolled_back.native_execution_id, None);
        assert!(
            ProviderDiscoveryCredentialCommitConfirmation::try_from(&rolled_back).is_err(),
            "a rolled-back reservation is not external-effect authority"
        );

        let error = reopened
            .start_provider_discovery_credential_install(
                &rolled_back.session_id,
                rolled_back.session_revision,
                &rolled_back.operation_id,
                &rolled_back.commit_attempt_id,
                &rolled_back.commit_plan_sha256,
                rolled_back
                    .native_execution_reservation_id
                    .as_deref()
                    .expect("rolled-back reservation B"),
            )
            .expect_err("a reopened Core must not start process-local reservation B");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        let error = reopened
            .commit_provider_discovery(&committing.session.id, Some(&confirmation_b))
            .expect_err("an externally available B cannot confirm a Prepared rollback");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        let error = reopened
            .reserve_provider_discovery_credential_install(
                &rolled_back.session_id,
                rolled_back.session_revision,
                &rolled_back.operation_id,
                &rolled_back.commit_attempt_id,
                &rolled_back.commit_plan_sha256,
            )
            .expect_err("a reopened Prepared reservation must not be reused");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);

        reopened
            .recover_provider_discovery(Utc::now())
            .expect("terminalize the unstarted rolled-back reservation");
        let interrupted = reopened
            .get_provider_discovery(&committing.session.id)
            .expect("load interrupted rolled-back reservation");
        assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
        drop(reopened);

        let reopened_after_recovery =
            open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
        let interrupted = reopened_after_recovery
            .get_provider_discovery(&committing.session.id)
            .expect("reload interrupted rolled-back reservation");
        assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
        let abandoned_b = reopened_after_recovery
            .storage()
            .get_discovery_native_credential_execution(&rolled_back.operation_id)
            .expect("reload append-only abandoned reservation B")
            .expect("abandoned reservation B remains auditable");
        assert_eq!(
            Some(abandoned_b.physical_authority_id),
            rolled_back.native_execution_reservation_id
        );
        assert_eq!(abandoned_b.store_started_at, None);
        let restarted = reopened_after_recovery
            .continue_provider_discovery(
                &committing.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    interrupted.session.revision,
                    ProviderDiscoveryAction::RestartInterrupted,
                )
                .expect("restart rolled-back reservation action"),
                None,
            )
            .expect("restart with a new semantic operation");
        let next_prepared = reopened_after_recovery
            .get_provider_discovery_credential_install_context(&restarted.session.id)
            .expect("load new prepared credential operation");
        assert_ne!(next_prepared.operation_id, rolled_back.operation_id);
        let reserved_c = reserve_credential_install(&reopened_after_recovery, &next_prepared);
        assert_ne!(
            reserved_c.native_execution_reservation_id,
            rolled_back.native_execution_reservation_id
        );
    }

    #[test]
    fn repeated_pre_store_recovery_does_not_leak_process_local_reservations() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core with native recovery ownership");
        let committing =
            prepare_no_network_credential_commit(&core, "credential-reservation-cleanup");
        let mut reserved_ids = BTreeSet::new();

        for cycle in 0..4 {
            let prepared = core
                .get_provider_discovery_credential_install_context(&committing.session.id)
                .expect("load prepared credential operation");
            let reserved = reserve_credential_install(&core, &prepared);
            assert!(
                reserved_ids.insert(
                    reserved
                        .native_execution_reservation_id
                        .clone()
                        .expect("fresh physical reservation"),
                ),
                "every restarted semantic operation must reserve a fresh physical id"
            );
            assert_eq!(core.pending_discovery_credential_reservation_count(), 1);

            core.recover_provider_discovery(Utc::now())
                .expect("recover abandoned pre-store reservation");
            assert_eq!(core.pending_discovery_credential_reservation_count(), 0);
            let interrupted = core
                .get_provider_discovery(&committing.session.id)
                .expect("load interrupted pre-store reservation");
            assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
            if cycle < 3 {
                core.continue_provider_discovery(
                    &committing.session.id,
                    provider_discovery_action_envelope(
                        DiscoveryActionId::new(),
                        interrupted.session.revision,
                        ProviderDiscoveryAction::RestartInterrupted,
                    )
                    .expect("restart abandoned reservation action"),
                    None,
                )
                .expect("restart abandoned pre-store reservation");
            }
        }
        assert_eq!(reserved_ids.len(), 4);
    }

    #[test]
    fn prepared_reservation_cancel_validates_revision_before_process_cleanup() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core with native recovery ownership");
        let committing =
            prepare_no_network_credential_commit(&core, "credential-reservation-cancel-cleanup");
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("load prepared credential operation");
        let reserved = reserve_credential_install(&core, &prepared);
        assert_eq!(core.pending_discovery_credential_reservation_count(), 1);

        core.cancel_provider_discovery(
            &committing.session.id,
            reserved.session_revision.saturating_add(1),
        )
        .expect_err("stale cancellation must fail before reservation cleanup");
        assert_eq!(core.pending_discovery_credential_reservation_count(), 1);

        let cancelled = core
            .cancel_provider_discovery(&committing.session.id, reserved.session_revision)
            .expect("cancel exact prepared reservation");
        assert_eq!(core.pending_discovery_credential_reservation_count(), 0);
        assert!(matches!(
            cancelled.session.state,
            DiscoveryState::Interrupted | DiscoveryState::Cancelled
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn prepared_reserved_cancel_crash_recovers_abandonment_without_reusing_b() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core with native recovery ownership");
        let committing =
            prepare_no_network_credential_commit(&core, "prepared-cancel-crash-reservation");
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("load prepared credential operation");
        let reserved = reserve_credential_install(&core, &prepared);
        let reservation_b = reserved
            .native_execution_reservation_id
            .clone()
            .expect("reserved physical B");
        let committing_snapshot = core
            .get_provider_discovery(&committing.session.id)
            .expect("load committing discovery before cancellation");
        let cancelling = persist_unsettled_credential_cancel(&core, &committing_snapshot);
        assert!(cancelling.session.cancellation_pending);
        assert!(cancelling.session.revision > reserved.session_revision);
        drop(core);

        let reopened =
            open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
        reopened
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect_err("normal install context must reject cancellation-pending reservation");
        let recovery = reopened
            .get_provider_discovery_credential_install_recovery_context(&committing.session.id)
            .expect("load exact prepared cancellation recovery context");
        assert_eq!(
            recovery.operation_status,
            DiscoveryOperationStatus::Prepared
        );
        assert_eq!(
            recovery.native_execution_reservation_id.as_deref(),
            Some(reservation_b.as_str())
        );
        assert_eq!(recovery.native_execution_id, None);

        reopened
            .recover_provider_discovery(Utc::now())
            .expect("recover cancellation-pending prepared reservation");
        let interrupted = reopened
            .get_provider_discovery(&committing.session.id)
            .expect("reload recovered prepared cancellation");
        assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
        let abandoned = reopened
            .storage()
            .get_discovery_native_credential_execution(&recovery.operation_id)
            .expect("load abandoned prepared reservation")
            .expect("append-only reservation B remains auditable");
        assert_eq!(abandoned.physical_authority_id, reservation_b);
        assert_eq!(abandoned.store_started_at, None);
        reopened
            .start_provider_discovery_credential_install(
                &recovery.session_id,
                recovery.session_revision,
                &recovery.operation_id,
                &recovery.commit_attempt_id,
                &recovery.commit_plan_sha256,
                &abandoned.physical_authority_id,
            )
            .expect_err("abandoned reservation B cannot be reused after recovery");
    }

    #[test]
    fn started_cancel_crash_reopens_with_exact_b_for_compensation() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core with native recovery ownership");
        let committing =
            prepare_no_network_credential_commit(&core, "started-cancel-crash-authority");
        let credential_authority = core
            .get_provider_discovery_credential_lease_context(&committing.session.id)
            .expect("load immutable credential origin authority before compensation");
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("load prepared credential operation");
        let started = reserve_and_start_credential_install(&core, &prepared);
        let physical_b = native_execution_id(&started).to_owned();
        let cancelling = core
            .cancel_provider_discovery(&committing.session.id, started.session_revision)
            .expect("persist Started cancellation intent");
        assert!(cancelling.session.cancellation_pending);
        assert!(cancelling.session.revision > started.session_revision);
        drop(core);

        let reopened =
            open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
        reopened
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect_err("normal install context must reject cancellation-pending Started WAL");
        let recovery = reopened
            .get_provider_discovery_credential_install_recovery_context(&committing.session.id)
            .expect("load exact Started cancellation recovery context");
        assert_eq!(recovery.operation_status, DiscoveryOperationStatus::Started);
        assert_eq!(
            recovery.native_execution_reservation_id.as_deref(),
            Some(physical_b.as_str())
        );
        assert_eq!(
            recovery.native_execution_id.as_deref(),
            Some(physical_b.as_str())
        );

        reopened
            .commit_provider_discovery(&committing.session.id, None)
            .expect_err("cancellation-pending Started WAL must enter compensation");
        let authority = reopened
            .get_provider_discovery_credential_compensation_authority(&committing.session.id)
            .expect("load exact physical compensation authority B");
        assert_eq!(authority.operation_id, started.operation_id);
        assert_eq!(authority.native_execution_id, physical_b);
        assert_eq!(
            authority.credential_api_origin,
            credential_authority.credential_api_origin
        );
        assert_eq!(
            authority.credential_origin_approval_id,
            credential_authority.credential_origin_approval_id
        );
        assert_eq!(
            authority.credential_origin_grant_sha256,
            credential_authority.credential_origin_grant_sha256
        );
        assert_eq!(
            authority.connection_binding_sha256,
            credential_authority.connection_binding_sha256
        );
        drop(reopened);

        let reopened =
            open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
        assert_eq!(
            reopened
                .get_provider_discovery_credential_compensation_authority(&committing.session.id)
                .expect("reload exact physical compensation authority B"),
            authority
        );
    }

    #[test]
    fn recovery_context_rejects_forged_cancel_revision_and_receipt_history() {
        let revision_root = tempdir().expect("temporary revision-tamper Core root");
        let revision_session = seed_started_cancellation_for_tamper(
            revision_root.path(),
            "started-cancel-revision-tamper",
        );
        let revision_database = active_test_database_path(revision_root.path());
        let revision_connection =
            rusqlite::Connection::open(&revision_database).expect("open revision-tamper database");
        assert_eq!(
            revision_connection
                .execute(
                    "UPDATE provider_discovery_sessions
                     SET revision = revision + 1,
                         next_event_sequence = next_event_sequence + 1
                     WHERE id = ?1",
                    [revision_session.as_str()],
                )
                .expect("forge unreceipted session revision"),
            1
        );
        drop(revision_connection);
        let revision_core = open_core_after_drop(
            revision_root.path(),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        );
        let revision_error = revision_core
            .get_provider_discovery_credential_install_recovery_context(&revision_session)
            .expect_err("unreceipted cancellation revision must not authorize recovery");
        assert_eq!(revision_error.code, CoreErrorCode::StorageCorrupted);

        let receipt_root = tempdir().expect("temporary receipt-tamper Core root");
        let receipt_session = seed_started_cancellation_for_tamper(
            receipt_root.path(),
            "started-cancel-receipt-tamper",
        );
        let receipt_database = active_test_database_path(receipt_root.path());
        let receipt_connection =
            rusqlite::Connection::open(&receipt_database).expect("open receipt-tamper database");
        receipt_connection
            .execute_batch("DROP TRIGGER provider_discovery_receipt_no_update;")
            .expect("open immutable receipt for synthetic tamper");
        assert_eq!(
            receipt_connection
                .execute(
                    "UPDATE provider_discovery_action_receipts
                     SET action_kind = 'approve_review'
                     WHERE session_id = ?1 AND action_kind = 'cancel'",
                    [receipt_session.as_str()],
                )
                .expect("forge cancellation receipt kind"),
            1
        );
        drop(receipt_connection);
        let receipt_core = open_core_after_drop(
            receipt_root.path(),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        );
        let receipt_error = receipt_core
            .get_provider_discovery_credential_install_recovery_context(&receipt_session)
            .expect_err("forged cancellation receipt must not authorize recovery");
        assert_eq!(receipt_error.code, CoreErrorCode::StorageCorrupted);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn restarted_atomic_credential_install_uses_exact_restart_receipt_authority() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core with native recovery ownership");
        let committing =
            prepare_no_network_credential_commit(&core, "credential-install-restart-authority");
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("initial credential install context");
        let first_started = reserve_and_start_credential_install(&core, &prepared);
        let first_interrupted = core
            .attest_provider_discovery_credential_install_no_effect(
                &committing.session.id,
                &first_started.operation_id,
                &prepared.commit_attempt_id,
                &prepared.commit_plan_sha256,
                native_execution_id(&first_started),
            )
            .expect("attest initial credential installation had no effect");
        assert_eq!(first_interrupted.session.state, DiscoveryState::Interrupted);

        let restarted = core
            .continue_provider_discovery(
                &committing.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    first_interrupted.session.revision,
                    ProviderDiscoveryAction::RestartInterrupted,
                )
                .expect("restart-interrupted action"),
                None,
            )
            .expect("restart interrupted credential commit");
        assert_eq!(restarted.session.state, DiscoveryState::Committing);

        let retry_prepared = core
            .get_provider_discovery_credential_install_context(&restarted.session.id)
            .expect("retry credential install context");
        assert_ne!(retry_prepared.operation_id, first_started.operation_id);
        assert_eq!(retry_prepared.session_revision, restarted.session.revision);
        assert_eq!(retry_prepared.commit_attempt_id, prepared.commit_attempt_id);
        assert_eq!(
            retry_prepared.commit_plan_sha256,
            prepared.commit_plan_sha256
        );
        assert_eq!(
            retry_prepared.operation_status,
            DiscoveryOperationStatus::Prepared
        );
        assert_eq!(retry_prepared.native_execution_reservation_id, None);
        assert_eq!(retry_prepared.native_execution_id, None);

        let retry_reserved = reserve_credential_install(&core, &retry_prepared);
        let stale_start = core
            .start_provider_discovery_credential_install(
                &restarted.session.id,
                restarted.session.revision,
                &first_started.operation_id,
                &retry_reserved.commit_attempt_id,
                &retry_reserved.commit_plan_sha256,
                retry_reserved
                    .native_execution_reservation_id
                    .as_deref()
                    .expect("retry native execution reservation"),
            )
            .expect_err("a prior operation must not start the retry credential effect");
        assert_eq!(stale_start.code, CoreErrorCode::InvalidInput);

        let retry_started = start_reserved_credential_install(&core, &retry_reserved);
        assert_eq!(
            retry_started.operation_status,
            DiscoveryOperationStatus::Started
        );
        assert!(retry_started.native_execution_id.is_some());
        assert_ne!(
            retry_started.native_execution_id, first_started.native_execution_id,
            "a restarted semantic commit must mint a new physical native authority"
        );
        let stale_attestation = core
            .attest_provider_discovery_credential_install_no_effect(
                &restarted.session.id,
                &first_started.operation_id,
                &retry_started.commit_attempt_id,
                &retry_started.commit_plan_sha256,
                native_execution_id(&first_started),
            )
            .expect_err("a prior operation must not attest the retry credential slot");
        assert_eq!(stale_attestation.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            core.get_provider_discovery_credential_install_context(&restarted.session.id)
                .expect("reload retry after stale attestation")
                .operation_status,
            DiscoveryOperationStatus::Started
        );
        let retry_interrupted = core
            .attest_provider_discovery_credential_install_no_effect(
                &restarted.session.id,
                &retry_started.operation_id,
                &retry_started.commit_attempt_id,
                &retry_started.commit_plan_sha256,
                native_execution_id(&retry_started),
            )
            .expect("attest retry credential installation had no effect");
        assert_eq!(retry_interrupted.session.state, DiscoveryState::Interrupted);
        let first_attestation = core
            .storage()
            .get_discovery_native_no_effect_attestation(&first_started.operation_id)
            .expect("load initial native no-effect attestation")
            .expect("initial native no-effect attestation");
        assert_eq!(
            first_attestation.physical_authority_id,
            native_execution_id(&first_started)
        );
        let retry_attestation = core
            .storage()
            .get_discovery_native_no_effect_attestation(&retry_started.operation_id)
            .expect("load retry native no-effect attestation")
            .expect("retry native no-effect attestation");
        assert_eq!(
            retry_attestation.physical_authority_id,
            native_execution_id(&retry_started)
        );
        drop(core);

        let reopened = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
        assert_eq!(
            reopened
                .get_provider_discovery(&committing.session.id)
                .expect("load twice-interrupted discovery")
                .session
                .state,
            DiscoveryState::Interrupted
        );
        assert_eq!(
            reopened
                .storage()
                .get_discovery_native_no_effect_attestation(&retry_started.operation_id)
                .expect("load retry attestation after reopen")
                .expect("durable retry attestation"),
            retry_attestation
        );
    }

    #[test]
    fn restarted_atomic_commit_rejects_prior_operation_confirmation_before_publish() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core with native recovery ownership");
        let committing =
            prepare_no_network_credential_commit(&core, "credential-retry-operation-confirmation");
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("initial credential install context");
        let first_started = reserve_and_start_credential_install(&core, &prepared);
        let interrupted = core
            .attest_provider_discovery_credential_install_no_effect(
                &committing.session.id,
                &first_started.operation_id,
                &first_started.commit_attempt_id,
                &first_started.commit_plan_sha256,
                native_execution_id(&first_started),
            )
            .expect("attest initial operation had no effect");
        let restarted = core
            .continue_provider_discovery(
                &committing.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    interrupted.session.revision,
                    ProviderDiscoveryAction::RestartInterrupted,
                )
                .expect("restart action"),
                None,
            )
            .expect("restart credential commit");
        let retry_prepared = core
            .get_provider_discovery_credential_install_context(&restarted.session.id)
            .expect("retry credential install context");
        let retry_started = reserve_and_start_credential_install(&core, &retry_prepared);
        assert_ne!(first_started.operation_id, retry_started.operation_id);
        assert_eq!(
            first_started.commit_attempt_id,
            retry_started.commit_attempt_id
        );
        drop(core);

        let reopened =
            open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
        let reopened_retry = reopened
            .get_provider_discovery_credential_install_context(&restarted.session.id)
            .expect("reload exact started retry context");
        assert_eq!(reopened_retry, retry_started);

        let stale_confirmation = credential_commit_confirmation(&first_started);
        let error = reopened
            .commit_provider_discovery(&restarted.session.id, Some(&stale_confirmation))
            .expect_err("a prior operation's observed slot must not publish the retry graph");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            reopened
                .get_provider_discovery_credential_install_context(&restarted.session.id)
                .expect("reload retry after stale confirmation")
                .operation_status,
            DiscoveryOperationStatus::Started
        );
        assert!(
            reopened
                .list_provider_connections()
                .expect("list provider connections after stale confirmation")
                .iter()
                .all(|connection| connection.id != retry_started.connection_id)
        );

        let exact_confirmation = credential_commit_confirmation(&reopened_retry);
        let committed = reopened
            .commit_provider_discovery(&restarted.session.id, Some(&exact_confirmation))
            .expect("the exact retry operation confirmation publishes the graph");
        assert_eq!(committed.id, retry_started.connection_id);
    }

    #[test]
    fn restarted_atomic_compensation_keeps_retry_operation_physical_authority() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core with native recovery ownership");
        let committing =
            prepare_no_network_credential_commit(&core, "credential-retry-compensation-authority");
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("initial credential install context");
        let first_started = reserve_and_start_credential_install(&core, &prepared);
        let interrupted = core
            .attest_provider_discovery_credential_install_no_effect(
                &committing.session.id,
                &first_started.operation_id,
                &first_started.commit_attempt_id,
                &first_started.commit_plan_sha256,
                native_execution_id(&first_started),
            )
            .expect("attest initial operation had no effect");
        let restarted = core
            .continue_provider_discovery(
                &committing.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    interrupted.session.revision,
                    ProviderDiscoveryAction::RestartInterrupted,
                )
                .expect("restart action"),
                None,
            )
            .expect("restart credential commit");
        let retry_prepared = core
            .get_provider_discovery_credential_install_context(&restarted.session.id)
            .expect("retry credential install context");
        let retry_started = reserve_and_start_credential_install(&core, &retry_prepared);
        let cancelled = core
            .cancel_provider_discovery(&restarted.session.id, restarted.session.revision)
            .expect("request retry cancellation");
        assert!(cancelled.session.cancellation_pending);
        core.commit_provider_discovery(&restarted.session.id, None)
            .expect_err("started retry cancellation enters compensation");

        let authority = core
            .get_provider_discovery_credential_compensation_authority(&restarted.session.id)
            .expect("load retry compensation authority");
        assert_eq!(authority.operation_id, retry_started.operation_id);
        assert_ne!(authority.operation_id, first_started.operation_id);
        assert_eq!(
            Some(authority.native_execution_id.clone()),
            retry_started.native_execution_id
        );
        assert_ne!(
            Some(authority.native_execution_id.clone()),
            first_started.native_execution_id
        );
        assert_eq!(authority.commit_attempt_id, retry_started.commit_attempt_id);
        assert_eq!(
            authority.connection_binding_sha256,
            retry_started.connection_binding_sha256
        );
        drop(core);

        let reopened =
            open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
        assert_eq!(
            reopened
                .get_provider_discovery_credential_compensation_authority(&restarted.session.id)
                .expect("reload retry compensation authority"),
            authority
        );
    }

    #[test]
    fn credential_durability_unknown_is_exact_and_survives_reopen_recovery() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core with native recovery ownership");
        let committing =
            prepare_no_network_credential_commit(&core, "credential-durability-unknown");
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("load prepared credential operation");
        let started = reserve_and_start_credential_install(&core, &prepared);
        let native_execution_id = native_execution_id(&started).to_owned();

        core.mark_provider_discovery_credential_install_durability_unknown(
            &started.session_id,
            started.session_revision + 1,
            &started.operation_id,
            &started.commit_attempt_id,
            &started.commit_plan_sha256,
            &native_execution_id,
            &started.connection_id,
            &started.connection_binding_sha256,
        )
        .expect_err("a stale session revision cannot settle native authority");
        assert_eq!(
            core.get_provider_discovery_credential_install_context(&started.session_id)
                .expect("stale settlement leaves active operation intact")
                .operation_status,
            DiscoveryOperationStatus::Started
        );

        let unknown = core
            .mark_provider_discovery_credential_install_durability_unknown(
                &started.session_id,
                started.session_revision,
                &started.operation_id,
                &started.commit_attempt_id,
                &started.commit_plan_sha256,
                &native_execution_id,
                &started.connection_id,
                &started.connection_binding_sha256,
            )
            .expect("settle exact native durability failure");
        assert_eq!(unknown.session.state, DiscoveryState::UnknownOutcome);
        assert_eq!(
            unknown.session.unknown_operation,
            Some(DiscoveryOperationKind::AtomicCommit)
        );
        assert!(
            core.list_provider_connections()
                .expect("list connections before reopen")
                .iter()
                .all(|connection| connection.id != started.connection_id),
            "visible native bytes must not publish or adopt the provider graph"
        );
        drop(core);

        let reopened =
            open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
        reopened
            .recover_provider_discovery(Utc::now())
            .expect("generic recovery preserves explicit unknown outcome");
        let preserved = reopened
            .get_provider_discovery(&started.session_id)
            .expect("reload durability-unknown discovery");
        assert_eq!(preserved.session.state, DiscoveryState::UnknownOutcome);
        assert!(
            reopened
                .list_provider_discovery_credential_recovery_candidates()
                .expect("list recovery candidates")
                .iter()
                .all(|candidate| candidate.session.id != started.session_id),
            "startup must not turn visibility into a new install authority"
        );
        reopened
            .commit_provider_discovery(
                &started.session_id,
                Some(&credential_commit_confirmation(&started)),
            )
            .expect_err("unknown durability cannot be adopted by a later commit call");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn native_recovery_owner_reconciles_credential_wal_without_network() {
        #[derive(Debug, Clone, Copy)]
        enum WalState {
            Prepared,
            Started,
        }

        #[derive(Debug, Clone, Copy)]
        enum VaultState {
            Missing,
            Available,
        }

        for (case_index, wal_state, vault_state) in [
            (0, WalState::Prepared, VaultState::Missing),
            (1, WalState::Prepared, VaultState::Available),
            (2, WalState::Started, VaultState::Missing),
            (3, WalState::Started, VaultState::Available),
        ] {
            let root = tempdir().expect("temporary Core root");
            let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
            let committing = prepare_no_network_credential_commit(
                &core,
                &format!("native-recovery-no-network-{case_index}"),
            );
            let prepared = core
                .get_provider_discovery_credential_install_context(&committing.session.id)
                .expect("prepared credential install context");
            if matches!(wal_state, WalState::Started) {
                reserve_and_start_credential_install(&core, &prepared);
            }
            drop(core);

            let reopened =
                open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
            let preserved = reopened
                .get_provider_discovery(&committing.session.id)
                .expect("load preserved credential commit");
            assert_eq!(preserved.session.state, DiscoveryState::Committing);
            assert_ne!(preserved.session.state, DiscoveryState::UnknownOutcome);
            assert!(
                reopened
                    .list_provider_discovery_credential_recovery_candidates()
                    .expect("list credential recovery candidates")
                    .iter()
                    .any(|candidate| candidate.session.id == committing.session.id)
            );
            let context = reopened
                .get_provider_discovery_credential_install_context(&committing.session.id)
                .expect("load preserved credential install context");
            assert_eq!(
                context.operation_status,
                match wal_state {
                    WalState::Prepared => DiscoveryOperationStatus::Prepared,
                    WalState::Started => DiscoveryOperationStatus::Started,
                }
            );
            assert_eq!(
                context.native_execution_id.is_some(),
                matches!(wal_state, WalState::Started),
                "only a durably started WAL has physical native authority"
            );
            assert_eq!(
                context.native_execution_reservation_id.is_some(),
                matches!(wal_state, WalState::Started),
                "these recovery fixtures reserve only immediately before starting"
            );

            let terminal = match (wal_state, vault_state) {
                (WalState::Started, VaultState::Available) => {
                    let confirmation = credential_commit_confirmation(&context);
                    reopened
                        .commit_provider_discovery(&committing.session.id, Some(&confirmation))
                        .expect("resume exact started credential commit");
                    reopened
                        .get_provider_discovery(&committing.session.id)
                        .expect("load resumed credential commit")
                }
                (WalState::Started, VaultState::Missing) => reopened
                    .attest_provider_discovery_credential_install_no_effect(
                        &committing.session.id,
                        &context.operation_id,
                        &context.commit_attempt_id,
                        &context.commit_plan_sha256,
                        native_execution_id(&context),
                    )
                    .expect("attest exact missing credential slot"),
                (WalState::Prepared, VaultState::Missing | VaultState::Available) => {
                    reopened
                        .recover_provider_discovery(Utc::now())
                        .expect("conservatively recover prepared credential operation");
                    reopened
                        .get_provider_discovery(&committing.session.id)
                        .expect("load interrupted prepared credential operation")
                }
            };
            let expected_state = match (wal_state, vault_state) {
                (WalState::Started, VaultState::Available) => DiscoveryState::Ready,
                _ => DiscoveryState::Interrupted,
            };
            assert_eq!(terminal.session.state, expected_state);
            assert_ne!(terminal.session.state, DiscoveryState::UnknownOutcome);
            assert!(
                reopened
                    .list_provider_discovery_credential_recovery_candidates()
                    .expect("list reconciled credential recovery candidates")
                    .iter()
                    .all(|candidate| candidate.session.id != committing.session.id)
            );
            let attestation = reopened
                .storage()
                .get_discovery_native_no_effect_attestation(&context.operation_id)
                .expect("load native recovery attestation");
            assert_eq!(
                attestation.is_some(),
                matches!(
                    (wal_state, vault_state),
                    (WalState::Started, VaultState::Missing)
                )
            );
            if let Some(attestation) = &attestation {
                assert_eq!(
                    attestation.physical_authority_id,
                    native_execution_id(&context)
                );
                assert_eq!(attestation.session_id, committing.session.id);
                assert_eq!(attestation.commit_attempt_id, context.commit_attempt_id);
                assert_eq!(attestation.commit_plan_sha256, context.commit_plan_sha256);
                assert_eq!(attestation.connection_id, context.connection_id);
            }
            drop(reopened);

            let final_reopen =
                open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
            assert_eq!(
                final_reopen
                    .get_provider_discovery(&committing.session.id)
                    .expect("load reconciled discovery")
                    .session
                    .state,
                expected_state
            );
            assert_eq!(
                final_reopen
                    .storage()
                    .get_discovery_native_no_effect_attestation(&context.operation_id)
                    .expect("load native attestation after final reopen"),
                attestation
            );
        }
    }

    #[test]
    fn core_recovery_owner_conservatively_classifies_started_credential_wal_without_network() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
        let committing = prepare_no_network_credential_commit(&core, "core-recovery-no-network");
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("prepared credential install context");
        let started = reserve_and_start_credential_install(&core, &prepared);
        let error = core
            .attest_provider_discovery_credential_install_no_effect(
                &committing.session.id,
                &prepared.operation_id,
                &prepared.commit_attempt_id,
                &prepared.commit_plan_sha256,
                native_execution_id(&started),
            )
            .expect_err("default Core must not claim native vault provenance");
        assert_eq!(error.code, CoreErrorCode::PermissionDenied);
        assert_eq!(
            core.get_provider_discovery_credential_install_context(&committing.session.id)
                .expect("reload rejected credential attestation context")
                .operation_status,
            DiscoveryOperationStatus::Started
        );
        drop(core);

        let reopened = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
        let recovered = reopened
            .get_provider_discovery(&committing.session.id)
            .expect("load conservatively recovered discovery");
        assert_eq!(recovered.session.state, DiscoveryState::UnknownOutcome);
        assert_eq!(
            recovered.session.unknown_operation,
            Some(DiscoveryOperationKind::AtomicCommit)
        );
        assert!(
            reopened
                .get_provider_discovery_credential_lease_context(&committing.session.id)
                .is_err(),
            "unknown external outcomes must never authorize a pre-commit credential lease"
        );
        assert!(
            reopened
                .list_provider_discovery_credential_recovery_candidates()
                .expect("list credential recovery candidates")
                .iter()
                .all(|candidate| candidate.session.id != committing.session.id)
        );
    }

    fn assistant_manifest_and_claims() -> (ProviderManifest, Vec<EvidenceClaim>) {
        let mut manifest = AdapterRegistry::built_in_templates()
            .unwrap()
            .into_iter()
            .find(|template| template.api_family == ApiFamily::OpenAiChatCompletions)
            .unwrap()
            .default_manifest;
        manifest.default_api_origin =
            Some(CanonicalOrigin::parse("https://api.assistant.example").unwrap());
        manifest.sources = vec![lorepia_domain::ManifestSource {
            kind: lorepia_domain::ManifestSourceKind::OfficialDocumentation,
            url: HttpUrl::parse("https://docs.assistant.example/").unwrap(),
            content_sha256: Some("a".repeat(64)),
        }];
        manifest.endpoints.models = None;
        manifest.decoders.streaming = None;
        manifest.parameters.clear();
        let fields = [
            (
                DraftField::ApiFamily,
                api_family_slug(manifest.api_family).to_owned(),
            ),
            (
                DraftField::DefaultApiOrigin,
                manifest
                    .default_api_origin
                    .as_ref()
                    .unwrap()
                    .as_str()
                    .to_owned(),
            ),
            (
                DraftField::Auth,
                serde_json::to_string(&manifest.auth).unwrap(),
            ),
            (
                DraftField::GenerateEndpoint,
                endpoint_claim(
                    manifest.endpoints.generate.method,
                    manifest.endpoints.generate.path.as_str(),
                ),
            ),
            (
                DraftField::ResponseDecoder,
                decoder_slug(manifest.decoders.response).to_owned(),
            ),
        ];
        let claims = fields
            .into_iter()
            .map(|(field, value)| EvidenceClaim::new(field, value).unwrap())
            .collect();
        (manifest, claims)
    }

    fn seed_assistant_route(core: &crate::Core) {
        let template = core
            .list_provider_templates()
            .unwrap()
            .into_iter()
            .find(|template| template.api_family == ApiFamily::OpenAiChatCompletions)
            .unwrap();
        let api_origin = CanonicalOrigin::parse("https://api.openai.com").unwrap();
        let connection = core
            .create_provider_connection(ProviderConnectionDraft {
                id: ProviderConnectionId::from("assistant-recovery-provider"),
                template_id: template.id,
                template_version: template.manifest_version,
                display_name: "Assistant recovery provider".to_owned(),
                api_origin: api_origin.clone(),
                api_base_path: Some(EndpointPath::parse("/v1").unwrap()),
                network_mode: ProviderNetworkMode::Public,
                values: vec![lorepia_domain::ConnectionConfigEntry {
                    key: "api_base_url".to_owned(),
                    value: ConnectionConfigValue::Text(format!("{}/v1", api_origin.as_str())),
                }],
                approved_credential_origin: Some(api_origin),
                local_network_approval: None,
                timeout_seconds: 5,
            })
            .unwrap();
        let now = Utc::now();
        core.upsert_model_route(ModelRoute {
            id: ModelRouteId::from("assistant-route"),
            connection_id: connection.id,
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: "assistant-model".to_owned(),
            display_name: Some("Assistant model".to_owned()),
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::Legacy,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: now,
            last_seen_at: Some(now),
        })
        .unwrap();
    }

    #[allow(clippy::too_many_lines)]
    fn seed_ready_assistant(root: &std::path::Path) -> (crate::Core, DiscoverySessionId) {
        let core = crate::Core::open(crate::CoreConfig::new(root)).unwrap();
        seed_assistant_route(&core);
        let storage = core.storage();
        let session_id = DiscoverySessionId::from(Uuid::new_v4().to_string());
        let input = SanitizedDiscoveryInput {
            connection_id: ProviderConnectionId::from("assistant-recovery-connection"),
            display_name: "Assistant recovery".to_owned(),
            site_url: HttpUrl::parse("https://docs.assistant.example/").unwrap(),
            docs_url: None,
            credential_ref: None,
            preferred_assistant: Some(ModelRouteId::from("assistant-route")),
            connection_options: ProviderDiscoveryConnectionOptions::default(),
            supplied_evidence_ids: Vec::new(),
        };
        let initial = ProviderDiscoverySession::new(session_id.clone(), input).unwrap();
        let mut draft = DiscoveryWorkingDraft::new(DiscoverySourceIntent::Site);
        let evidence_id = EvidenceId::from("assistant-recovery-evidence");
        let begin = initial
            .apply(
                &provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    0,
                    ProviderDiscoveryAction::Begin,
                )
                .unwrap(),
            )
            .unwrap();
        storage
            .begin_discovery_session(
                &initial,
                &DiscoveryTransitionWrite {
                    transition: begin,
                    draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft).unwrap()),
                    review: DiscoveryJsonUpdate::Clear,
                    new_evidence: Vec::new(),
                    new_candidates: Vec::new(),
                    approval: None,
                    new_operation_id: Some(DiscoveryOperationId::new()),
                    completed_operation: None,
                    prepared_commit: None,
                    provider_graph: None,
                    occurred_at: Utc::now(),
                },
            )
            .unwrap();
        let orchestrator = core.provider_discovery();

        let mut snapshot = orchestrator.get(&session_id).unwrap();
        let operation = storage
            .get_current_discovery_operation(&session_id)
            .unwrap()
            .unwrap();
        storage
            .mark_discovery_operation_started(&operation.id, Utc::now())
            .unwrap();
        draft = hydrate_working_draft(&snapshot).unwrap();
        let (_, claims) = assistant_manifest_and_claims();
        draft
            .assistant_evidence_claims
            .insert(evidence_id.clone(), claims);
        orchestrator
            .persist_operation_completion(
                &snapshot,
                &operation.id,
                &mut draft,
                ProviderDiscoveryAction::KnownProviderCandidatesResolved { candidate_count: 0 },
                DurableOperationOutcome::Succeeded,
                Vec::new(),
                Vec::new(),
                DiscoveryJsonUpdate::Preserve,
            )
            .unwrap();

        snapshot = orchestrator.get(&session_id).unwrap();
        let operation = storage
            .get_current_discovery_operation(&session_id)
            .unwrap()
            .unwrap();
        storage
            .mark_discovery_operation_started(&operation.id, Utc::now())
            .unwrap();
        draft = hydrate_working_draft(&snapshot).unwrap();
        draft.evidence_ids = vec![evidence_id.clone()];
        let evidence = DiscoveryEvidenceRecord {
            id: evidence_id,
            session_id: session_id.clone(),
            kind: DiscoveryEvidenceKind::PlainTextDocument,
            source_url: HttpUrl::parse("https://docs.assistant.example/").unwrap(),
            content_sha256: "a".repeat(64),
            extracted_json: json!({"summary": "bounded official provider documentation"}),
            fetched_at: Utc::now(),
        };
        orchestrator
            .persist_operation_completion(
                &snapshot,
                &operation.id,
                &mut draft,
                ProviderDiscoveryAction::DocumentsFetched { evidence_count: 1 },
                DurableOperationOutcome::Succeeded,
                vec![evidence],
                Vec::new(),
                DiscoveryJsonUpdate::Preserve,
            )
            .unwrap();

        snapshot = orchestrator.get(&session_id).unwrap();
        let operation = storage
            .get_current_discovery_operation(&session_id)
            .unwrap()
            .unwrap();
        storage
            .mark_discovery_operation_started(&operation.id, Utc::now())
            .unwrap();
        draft = hydrate_working_draft(&snapshot).unwrap();
        initialize_assistant(storage, &snapshot, &mut draft).unwrap();
        orchestrator
            .persist_operation_completion(
                &snapshot,
                &operation.id,
                &mut draft,
                ProviderDiscoveryAction::EvidenceExtracted {
                    resolution: DiscoveryEvidenceResolution::AssistantRecommended,
                },
                DurableOperationOutcome::Succeeded,
                Vec::new(),
                Vec::new(),
                DiscoveryJsonUpdate::Preserve,
            )
            .unwrap();

        snapshot = orchestrator.get(&session_id).unwrap();
        let proposal = orchestrator
            .approval_proposal(&session_id)
            .unwrap()
            .unwrap();
        orchestrator
            .continue_discovery(
                &session_id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    snapshot.session.revision,
                    ProviderDiscoveryAction::ApproveAssistant {
                        approval_id: proposal.id,
                        approval_grant_sha256: proposal.grant_sha256,
                    },
                )
                .unwrap(),
                None,
            )
            .unwrap();
        (core, session_id)
    }

    fn unresolved_question(id: impl Into<String>) -> UnresolvedQuestion {
        UnresolvedQuestion {
            id: id.into(),
            field: None,
            question: "Which current provider contract detail is still unresolved?".to_owned(),
            required_evidence: "One bounded official provider document excerpt.".to_owned(),
        }
    }

    fn seed_pending_unresolved_questions_tool(
        root: &std::path::Path,
        questions: Vec<UnresolvedQuestion>,
    ) -> (crate::Core, DiscoverySessionId) {
        let (core, session_id) = seed_ready_assistant(root);
        let orchestrator = core.provider_discovery();
        let snapshot = orchestrator.get(&session_id).unwrap();
        let mut draft = hydrate_working_draft(&snapshot).unwrap();
        let mut engine = restored_assistant(&draft).unwrap();
        let estimate = AssistantCallEstimate {
            input_tokens: 16,
            maximum_output_tokens: 64,
            maximum_cost_micro_units: 100,
        };
        engine.begin_turn(estimate).unwrap();
        assert!(matches!(
            engine
                .submit_turn(AssistantTurn::NeedMoreEvidence {
                    questions: questions.clone(),
                })
                .unwrap(),
            AssistantHostAction::RequestMoreEvidence { .. }
        ));
        engine.continue_after_more_evidence().unwrap();
        engine.begin_turn(estimate).unwrap();
        assert!(matches!(
            engine
                .submit_turn(AssistantTurn::CallTool {
                    call: AssistantToolCall::ShowUnresolvedQuestions,
                })
                .unwrap(),
            AssistantHostAction::ExecuteTool {
                call: AssistantToolCall::ShowUnresolvedQuestions,
                ..
            }
        ));
        synchronize_assistant_snapshot(&mut draft, &engine);
        orchestrator
            .persist_assistant_checkpoint(
                &snapshot,
                &draft,
                DiscoveryAssistantCheckpoint::AwaitingToolResult,
            )
            .unwrap();
        (core, session_id)
    }

    #[test]
    fn show_unresolved_questions_returns_exact_canonical_durable_ids() {
        let root = tempdir().unwrap();
        let questions = vec![
            unresolved_question("question-01"),
            unresolved_question("question-02"),
        ];
        let (core, session_id) = seed_pending_unresolved_questions_tool(root.path(), questions);

        let result = core
            .provider_discovery()
            .execute_assistant_tool(&session_id, &AssistantToolCall::ShowUnresolvedQuestions)
            .unwrap();

        assert_eq!(
            result,
            AssistantToolResult::UnresolvedQuestions {
                question_ids: vec!["question-01".to_owned(), "question-02".to_owned()],
            }
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn show_unresolved_questions_rejects_wrong_session_stale_or_invalid_durable_sets() {
        let root = tempdir().unwrap();
        let questions = vec![
            unresolved_question("question-01"),
            unresolved_question("question-02"),
        ];
        let (core, session_id) = seed_pending_unresolved_questions_tool(root.path(), questions);
        let orchestrator = core.provider_discovery();
        let current = orchestrator.get(&session_id).unwrap();
        let draft = hydrate_working_draft(&current).unwrap();
        let assert_rejected = |requested_session_id: &DiscoverySessionId,
                               observed_revision: u64,
                               candidate: &DiscoveryWorkingDraft| {
            let error = ProviderDiscoveryOrchestrator::validated_assistant_unresolved_question_ids(
                requested_session_id,
                observed_revision,
                &current,
                candidate,
            )
            .unwrap_err();
            assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        };

        assert_rejected(
            &DiscoverySessionId::from("another-session"),
            current.session.revision,
            &draft,
        );
        assert_rejected(
            &session_id,
            current.session.revision.saturating_sub(1),
            &draft,
        );

        let mut empty = draft.clone();
        empty.assistant_more_evidence_questions.clear();
        assert_rejected(&session_id, current.session.revision, &empty);

        let mut too_many = draft.clone();
        too_many.assistant_more_evidence_questions = (0..129)
            .map(|index| unresolved_question(format!("question-{index:03}")))
            .collect();
        assert_rejected(&session_id, current.session.revision, &too_many);

        let mut oversized_text = draft.clone();
        oversized_text.assistant_more_evidence_questions[0].question = "x".repeat(2 * 1024 + 1);
        assert_rejected(&session_id, current.session.revision, &oversized_text);

        let mut oversized_result = draft.clone();
        oversized_result.assistant_more_evidence_questions = (0..40)
            .map(|index| unresolved_question(format!("q-{index:03}-{}", "x".repeat(118))))
            .collect();
        assert_rejected(&session_id, current.session.revision, &oversized_result);

        let mut malformed = draft.clone();
        malformed.assistant_more_evidence_questions[0].id = "question with spaces".to_owned();
        assert_rejected(&session_id, current.session.revision, &malformed);

        let mut duplicate = draft.clone();
        duplicate.assistant_more_evidence_questions[1].id = "question-01".to_owned();
        assert_rejected(&session_id, current.session.revision, &duplicate);

        let mut out_of_order = draft;
        out_of_order.assistant_more_evidence_questions.swap(0, 1);
        assert_rejected(&session_id, current.session.revision, &out_of_order);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn selected_assistant_route_uses_exact_family_plan_and_decodes_only_the_envelope() {
        let root = tempdir().unwrap();
        let (core, session_id) = seed_ready_assistant(root.path());
        let estimate = AssistantCallEstimate {
            input_tokens: 16,
            maximum_output_tokens: 2_048,
            maximum_cost_micro_units: 100,
        };
        let mut prompt = core
            .provider_discovery()
            .begin_assistant_turn(&session_id, estimate)
            .unwrap();
        prompt.allowed_api_families = vec![ApiFamily::OpenAiChatCompletions];
        let expected_turn = AssistantTurn::NeedMoreEvidence {
            questions: vec![unresolved_question("need-current-contract")],
        };
        let response = serde_json::to_string(&json!({"turn": &expected_turn})).unwrap();
        let (mut outside_manifest, _) = assistant_manifest_and_claims();
        outside_manifest.api_family = ApiFamily::AnthropicMessages;
        let outside_allowlist_turn = AssistantTurn::SubmitDraft {
            draft: Box::new(AssistantManifestDraft {
                manifest: outside_manifest,
                evidence_mappings: Vec::new(),
                conflicts: Vec::new(),
                unresolved_questions: Vec::new(),
                confidence: Vec::new(),
                summary: "This family is intentionally outside the prompt allowlist.".to_owned(),
            }),
        };
        let outside_allowlist_response =
            serde_json::to_string(&json!({"turn": outside_allowlist_turn})).unwrap();
        let expected_family_enum = prompt
            .allowed_api_families
            .iter()
            .map(|family| api_family_slug(*family))
            .collect::<Vec<_>>();
        let mut route = core
            .storage()
            .get_model_route(&ModelRouteId::from("assistant-route"))
            .unwrap();

        for family in [
            ApiFamily::OpenAiResponses,
            ApiFamily::OpenAiChatCompletions,
            ApiFamily::AnthropicMessages,
            ApiFamily::GeminiGenerateContent,
            ApiFamily::OllamaNative,
        ] {
            route.api_family = family;
            let plain_generate_called = Arc::new(AtomicBool::new(false));
            let captured_bodies = Arc::new(Mutex::new(Vec::new()));
            let provider = Arc::new(ConstrainedAssistantCaptureProvider {
                plain_generate_called: Arc::clone(&plain_generate_called),
                captured_bodies: Arc::clone(&captured_bodies),
                response: response.clone(),
            });
            let output = core
                .runtime_handle()
                .block_on(run_setup_assistant_provider_call(
                    provider,
                    &route,
                    &prompt,
                    estimate,
                    Some("borrowed-only-credential"),
                ))
                .unwrap();
            assert_eq!(output, expected_turn);
            assert!(!plain_generate_called.load(Ordering::SeqCst));

            let rejected_provider = Arc::new(ConstrainedAssistantCaptureProvider {
                plain_generate_called: Arc::clone(&plain_generate_called),
                captured_bodies: Arc::clone(&captured_bodies),
                response: outside_allowlist_response.clone(),
            });
            let error = core
                .runtime_handle()
                .block_on(run_setup_assistant_provider_call(
                    rejected_provider,
                    &route,
                    &prompt,
                    estimate,
                    None,
                ))
                .expect_err("target family outside the prompt allowlist must be rejected");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert!(error.recoverable);
            assert!(!plain_generate_called.load(Ordering::SeqCst));

            let captured = captured_bodies
                .lock()
                .expect("read setup-assistant capture");
            assert_eq!(captured.len(), 2);
            assert_eq!(captured[0].0, family);
            assert_eq!(captured[1], captured[0]);
            let body = &captured[0].1;
            let schema = match family {
                ApiFamily::OpenAiResponses => {
                    let format = &body["text"]["format"];
                    assert_eq!(format["type"], "json_schema");
                    assert_eq!(format["name"], "lorepia_setup_assistant_turn_v1");
                    assert_eq!(format["strict"], true);
                    &format["schema"]
                }
                ApiFamily::OpenAiChatCompletions => {
                    let format = &body["response_format"];
                    assert_eq!(format["type"], "json_schema");
                    assert_eq!(
                        format["json_schema"]["name"],
                        "lorepia_setup_assistant_turn_v1"
                    );
                    assert_eq!(format["json_schema"]["strict"], true);
                    &format["json_schema"]["schema"]
                }
                ApiFamily::AnthropicMessages => {
                    let format = &body["output_config"]["format"];
                    assert_eq!(format["type"], "json_schema");
                    &format["schema"]
                }
                ApiFamily::GeminiGenerateContent => {
                    assert_eq!(
                        body["generationConfig"]["responseMimeType"],
                        "application/json"
                    );
                    &body["generationConfig"]["responseJsonSchema"]
                }
                ApiFamily::OllamaNative => &body["format"],
            };
            assert_eq!(schema["type"], "object");
            assert_eq!(schema["additionalProperties"], false);
            assert_eq!(
                schema["$defs"]["api_family"]["enum"],
                json!(expected_family_enum)
            );
            assert!(
                !serde_json::to_string(body)
                    .unwrap()
                    .contains("borrowed-only-credential")
            );
        }
    }

    #[test]
    fn provider_without_internal_plan_support_fails_without_bare_generation_fallback() {
        let root = tempdir().unwrap();
        let (core, session_id) = seed_ready_assistant(root.path());
        let estimate = AssistantCallEstimate {
            input_tokens: 16,
            maximum_output_tokens: 64,
            maximum_cost_micro_units: 100,
        };
        let prompt = core
            .provider_discovery()
            .begin_assistant_turn(&session_id, estimate)
            .unwrap();
        let route = core
            .storage()
            .get_model_route(&ModelRouteId::from("assistant-route"))
            .unwrap();
        let plain_generate_called = Arc::new(AtomicBool::new(false));
        let error = core
            .runtime_handle()
            .block_on(run_setup_assistant_provider_call(
                Arc::new(PlainOnlyAssistantProvider {
                    plain_generate_called: Arc::clone(&plain_generate_called),
                }),
                &route,
                &prompt,
                estimate,
                None,
            ))
            .unwrap_err();

        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
        assert!(!plain_generate_called.load(Ordering::SeqCst));
    }

    #[test]
    fn nested_schema_escape_secret_is_rejected_without_error_or_storage_persistence() {
        const SECRET_CANARY: &str = "sk-schema-escape-canary-abcdefghijklmnopqrstuvwxyz";

        let root = tempdir().unwrap();
        let (core, session_id) = seed_ready_assistant(root.path());
        let route = core
            .storage()
            .get_model_route(&ModelRouteId::from("assistant-route"))
            .unwrap();
        let estimate = AssistantCallEstimate {
            input_tokens: 16,
            maximum_output_tokens: 256,
            maximum_cost_micro_units: 100,
        };
        let response = json!({
            "turn": {
                "type": "need_more_evidence",
                "questions": [{
                    "id": "need-current-contract",
                    "field": {
                        "kind": "parameter",
                        "parameter_id": "temperature",
                        "credential": SECRET_CANARY
                    },
                    "question": "Which parameter contract is current?",
                    "required_evidence": "A current official parameter table."
                }]
            }
        })
        .to_string();
        let plain_generate_called = Arc::new(AtomicBool::new(false));
        let captured_bodies = Arc::new(Mutex::new(Vec::new()));
        let error = core
            .provider_discovery()
            .run_assistant_with_provider(
                &session_id,
                &route,
                Arc::new(ConstrainedAssistantCaptureProvider {
                    plain_generate_called: Arc::clone(&plain_generate_called),
                    captured_bodies: Arc::clone(&captured_bodies),
                    response,
                }),
                estimate,
                None,
            )
            .expect_err("nested schema escape must fail before assistant state submission");

        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert!(!format!("{error:?}").contains(SECRET_CANARY));
        assert!(!plain_generate_called.load(Ordering::SeqCst));
        assert_eq!(
            core.get_provider_discovery_assistant_resume_boundary(&session_id)
                .unwrap()
                .unwrap()
                .action,
            ProviderDiscoveryAssistantResumeAction::ApproveRetry
        );
        let snapshot = core.get_provider_discovery(&session_id).unwrap();
        assert!(!format!("{snapshot:?}").contains(SECRET_CANARY));
        assert!(
            captured_bodies
                .lock()
                .unwrap()
                .iter()
                .all(|(_, body)| !body.to_string().contains(SECRET_CANARY))
        );

        drop(core);
        assert_file_tree_omits(root.path(), SECRET_CANARY.as_bytes());
    }

    #[test]
    fn supplemental_public_sources_may_use_another_public_origin() {
        let input = input_with_options(
            "https://console.example/",
            ProviderDiscoveryConnectionOptions::default(),
        );
        let docs_origin = CanonicalOrigin::parse("https://docs.example").unwrap();
        let api_origin = CanonicalOrigin::parse("https://api.example").unwrap();
        assert!(additional_document_url_policy(&input, &docs_origin).is_ok());
        assert!(additional_curl_url_policy(&input, &api_origin).is_ok());
    }

    #[test]
    fn approved_lan_curl_is_exact_and_document_fetch_remains_disabled() {
        let options = approved_lan_options();
        let input = input_with_options("http://models.lan:8080/", options.clone());
        let approved_origin = CanonicalOrigin::parse("http://models.lan:8080").unwrap();
        let other_origin = CanonicalOrigin::parse("http://other.lan:8080").unwrap();

        assert!(additional_curl_url_policy(&input, &approved_origin).is_ok());
        assert!(additional_curl_url_policy(&input, &other_origin).is_err());
        assert!(additional_document_url_policy(&input, &approved_origin).is_err());

        assert!(
            ProviderDiscoverySource::curl(
                SecretCurlInput::new("curl http://models.lan:8080/v1/models".to_owned(),),
                options.clone(),
            )
            .is_ok()
        );
        assert!(
            ProviderDiscoverySource::curl(
                SecretCurlInput::new("curl http://other.lan:8080/v1/models".to_owned()),
                options,
            )
            .is_err()
        );
    }

    #[test]
    fn legacy_or_expired_lan_authority_cannot_reach_a_network_policy() {
        let mut legacy = approved_lan_options();
        legacy.local_network_approved_at = None;
        assert!(legacy.validate().is_ok(), "legacy records remain readable");
        assert!(
            ProviderDiscoverySource::curl(
                SecretCurlInput::new("curl http://models.lan:8080/v1/models".to_owned()),
                legacy.clone(),
            )
            .is_ok(),
            "pre-session cURL parsing does not itself perform a network effect"
        );
        assert!(discovery_url_policy(&legacy).is_err());

        let mut expired = approved_lan_options();
        expired.local_network_approved_at = Some(Utc::now() - chrono::Duration::hours(25));
        assert!(discovery_url_policy(&expired).is_err());
    }

    #[test]
    fn discovery_begin_issues_lan_authority_at_the_immutable_session_time() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
        let mut options = approved_lan_options();
        options.local_network_approved_at = Some(Utc::now() - chrono::Duration::hours(48));
        let snapshot = core
            .provider_discovery()
            .begin_with_credential_authority(
                input_with_options("http://models.lan:8080/", options),
                ProviderDiscoverySource::known_provider_id(ProviderTemplateId::from(
                    "unknown-lan-template",
                )),
                None,
            )
            .expect("persist LAN discovery before local template lookup fails closed");

        assert_eq!(
            snapshot
                .session
                .input
                .connection_options
                .local_network_approved_at,
            Some(snapshot.created_at),
            "Core must overwrite caller time and bind LAN authority to session creation"
        );
    }

    #[test]
    fn approved_lan_graph_seed_does_not_refresh_session_authority() {
        let approved_at = Utc::now() - chrono::Duration::hours(1);
        let observed_at = approved_at + chrono::Duration::minutes(30);
        let mut options = approved_lan_options();
        options.local_network_approved_at = Some(approved_at);
        let session = ProviderDiscoverySession::new(
            DiscoverySessionId::from("approved-lan-authority-time"),
            input_with_options("http://models.lan:8080/", options),
        )
        .expect("approved LAN discovery session");
        let snapshot = DiscoverySessionSnapshot {
            session,
            active_operation_id: None,
            draft_json: None,
            review: None,
            created_at: approved_at,
            updated_at: approved_at,
        };
        let mut template =
            AdapterRegistry::built_in_template(BuiltInTemplateId::OllamaNative).unwrap();
        template.default_manifest.default_api_origin =
            Some(CanonicalOrigin::parse("http://models.lan:8080").unwrap());
        let mut draft = DiscoveryWorkingDraft::new(DiscoverySourceIntent::KnownProvider {
            template_id: template.id.clone(),
        });

        install_graph_seed(&snapshot, &mut draft, template, observed_at)
            .expect("install approved LAN graph seed");

        assert_eq!(
            draft.connection.expect("seeded connection").created_at,
            approved_at,
            "graph seeding must carry the immutable LAN approval issue time"
        );
    }

    #[test]
    fn initial_discovery_preserves_exact_bounded_openrouter_model_metadata() {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("OpenRouter template");
        let observed_at = Utc::now();
        let mut draft = DiscoveryWorkingDraft::new(DiscoverySourceIntent::KnownProvider {
            template_id: template.id.clone(),
        });
        draft.connection = Some(ProviderConnection {
            id: ProviderConnectionId::from("openrouter-initial-discovery"),
            template_id: template.id.clone(),
            template_version: template.manifest_version,
            display_name: "OpenRouter initial discovery".to_owned(),
            api_origin: CanonicalOrigin::parse("https://openrouter.ai").expect("OpenRouter origin"),
            config: ConnectionConfig::default(),
            credential_ref: None,
            credential_scope: None,
            timeout_seconds: 30,
            status: ConnectionStatus::Untested,
            created_at: observed_at,
            updated_at: observed_at,
        });
        draft.template = Some(template);

        let listed = lorepia_providers::ListedModel {
            model_id: "openai/exact-metadata-model".to_owned(),
            display_name: Some("Exact metadata model".to_owned()),
            max_input_tokens: Some(128_000),
            max_output_tokens: Some(16_384),
            supported_generation_methods: Vec::new(),
            capabilities: lorepia_providers::ListedModelCapabilities {
                supported: vec![
                    lorepia_providers::ListedModelCapability::Reasoning,
                    lorepia_providers::ListedModelCapability::ToolCalling,
                ],
                parameters: lorepia_providers::OpenRouterSupportedParameterSupport::Exact(vec![
                    lorepia_providers::OpenRouterSupportedParameter::MaxCompletionTokens,
                    lorepia_providers::OpenRouterSupportedParameter::Reasoning,
                    lorepia_providers::OpenRouterSupportedParameter::Temperature,
                    lorepia_providers::OpenRouterSupportedParameter::Tools,
                ]),
                reasoning: Some(lorepia_providers::ListedModelReasoningCapability {
                    supported_efforts: lorepia_providers::OpenRouterReasoningEffortSupport::Exact(
                        vec![
                            lorepia_providers::OpenRouterReasoningEffort::High,
                            lorepia_providers::OpenRouterReasoningEffort::Low,
                        ],
                    ),
                    default_effort: Some(lorepia_providers::OpenRouterReasoningEffort::High),
                    default_enabled: Some(true),
                    supports_max_tokens: Some(false),
                    mandatory: Some(false),
                }),
            },
            source: lorepia_providers::ModelRecordSource::ProviderApi,
            availability: ModelAvailability::Available,
        };

        apply_listed_models_to_draft(&mut draft, &[listed], observed_at)
            .expect("apply initial provider listing");

        assert_eq!(
            draft.connection.as_ref().unwrap().status,
            ConnectionStatus::Connected
        );
        assert_eq!(draft.routes.len(), 1);
        let route = &draft.routes[0];
        assert_eq!(route.metadata_source, ModelMetadataSource::ProviderApi);
        assert_eq!(route.metadata_observed_at, Some(observed_at));
        let metadata = route
            .raw_metadata
            .as_ref()
            .expect("normalized provider metadata");
        let metadata: Value =
            serde_json::from_str(metadata.as_str()).expect("normalized metadata JSON");
        assert_eq!(metadata["capabilities"]["parameters"]["kind"], "exact");
        assert_eq!(
            metadata["capabilities"]["reasoning"]["supported_efforts"]["values"],
            json!(["high", "low"])
        );
        assert_eq!(
            metadata["capabilities"]["reasoning"]["default_effort"],
            "high"
        );
        assert!(
            draft.observations.iter().any(|observation| {
                observation.model_route_id == route.id
                    && observation.key == lorepia_domain::CapabilityKey::Reasoning
                    && observation.source == lorepia_domain::ObservationSource::ProviderApi
            }),
            "initial discovery must retain provider API capability provenance"
        );
        assert!(
            draft
                .observations
                .iter()
                .all(|observation| observation.key != lorepia_domain::CapabilityKey::PromptCaching),
            "OpenRouter model metadata must not infer prompt caching"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn openrouter_discovery_commit_and_reopen_preserves_exact_bounded_model_metadata() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("OpenRouter template");

        let connection_id = ProviderConnectionId::from("openrouter-discovery-reopen");
        let input = SanitizedDiscoveryInput {
            connection_id: connection_id.clone(),
            display_name: "OpenRouter discovery reopen".to_owned(),
            site_url: HttpUrl::parse("https://openrouter.ai/").expect("OpenRouter site URL"),
            docs_url: None,
            credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
            preferred_assistant: None,
            connection_options: ProviderDiscoveryConnectionOptions::default(),
            supplied_evidence_ids: Vec::new(),
        };
        let selecting = core
            .begin_provider_discovery_known(input, template.id.clone())
            .expect("begin exact OpenRouter discovery");
        assert_eq!(
            selecting.session.state,
            DiscoveryState::AwaitingTemplateSelection
        );
        let candidate = core
            .list_provider_discovery_candidates(&selecting.session.id)
            .expect("list template candidates")
            .into_iter()
            .find(|candidate| {
                matches!(
                    &candidate.candidate.summary,
                    DiscoveryCandidateSummary::ProviderTemplate {
                        template_id,
                        template_version,
                    } if template_id == &template.id
                        && *template_version == template.manifest_version
                )
            })
            .expect("exact OpenRouter template candidate");
        let selected = core
            .continue_provider_discovery(
                &selecting.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    selecting.session.revision,
                    ProviderDiscoveryAction::SelectTemplate {
                        candidate_id: candidate.candidate.id,
                    },
                )
                .expect("select-template action"),
                None,
            )
            .expect("select exact OpenRouter template");
        assert_eq!(
            selected.session.state,
            DiscoveryState::AwaitingCredentialOriginApproval
        );
        let credential_proposal = core
            .get_provider_discovery_approval_proposal(&selected.session.id)
            .expect("load credential-origin proposal")
            .expect("credential-origin proposal");
        let listed = approve_credential_and_seed_model_listing(
            &core,
            &selected,
            credential_proposal.id,
            &[exact_openrouter_listed_model()],
        );
        assert_eq!(listed.session.state, DiscoveryState::AwaitingProbeConsent);
        let reviewed = core
            .continue_provider_discovery(
                &listed.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    listed.session.revision,
                    ProviderDiscoveryAction::SkipProbes,
                )
                .expect("skip-probes action"),
                None,
            )
            .expect("skip OpenRouter probes");
        assert_eq!(reviewed.session.state, DiscoveryState::AwaitingReview);
        let proposal = core
            .get_provider_discovery_review_proposal(&reviewed.session.id)
            .expect("load review proposal")
            .expect("OpenRouter review proposal");
        let expected_attempt_id = proposal.commit_attempt_id.clone();
        let expected_plan_sha256 = proposal.commit_plan_sha256.clone();
        let committing = core
            .continue_provider_discovery(
                &reviewed.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    reviewed.session.revision,
                    ProviderDiscoveryAction::ApproveReview {
                        approval_id: proposal.approval.id,
                        commit_attempt_id: expected_attempt_id.clone(),
                        commit_plan_sha256: expected_plan_sha256.clone(),
                        graph_sha256: proposal.review.graph_sha256,
                    },
                )
                .expect("approve-review action"),
                None,
            )
            .expect("approve OpenRouter review");
        assert_eq!(committing.session.state, DiscoveryState::Committing);
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("credential install context");
        assert_eq!(prepared.session_revision, committing.session.revision);
        assert_eq!(prepared.commit_attempt_id, expected_attempt_id);
        assert_eq!(prepared.commit_plan_sha256, expected_plan_sha256);
        assert_eq!(prepared.commit_phase, DiscoveryCommitPhase::Prepared);
        assert_eq!(
            prepared.operation_status,
            DiscoveryOperationStatus::Prepared
        );
        let started = reserve_and_start_credential_install(&core, &prepared);
        assert_eq!(started.operation_status, DiscoveryOperationStatus::Started);
        assert_eq!(started.commit_phase, DiscoveryCommitPhase::Prepared);
        let confirmation = credential_commit_confirmation(&started);
        core.commit_provider_discovery(&committing.session.id, Some(&confirmation))
            .expect("commit exact OpenRouter graph");
        drop(core);

        let reopened = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
        reopened
            .storage()
            .ensure_provider_credential_access_settled(&connection_id)
            .expect("reopened discovery credential authority remains settled");
        let routes = reopened
            .list_model_routes(&connection_id)
            .expect("list reopened OpenRouter routes");
        assert_eq!(routes.len(), 1);
        let route = &routes[0];
        assert_eq!(route.metadata_source, ModelMetadataSource::ProviderApi);
        assert!(route.metadata_observed_at.is_some());
        let raw_metadata = route
            .raw_metadata
            .as_ref()
            .expect("reopened normalized metadata");
        assert!(!raw_metadata.as_str().contains("future_model_metadata"));
        assert!(!raw_metadata.as_str().contains("future_reasoning_metadata"));
        assert!(!raw_metadata.as_str().contains("future-effort-v9"));
        let metadata: Value =
            serde_json::from_str(raw_metadata.as_str()).expect("reopened metadata JSON");
        assert_eq!(
            metadata["capabilities"]["parameters"],
            json!({
                "kind": "exact",
                "values": [
                    "logprobs",
                    "max_completion_tokens",
                    "max_tokens",
                    "parallel_tool_calls",
                    "reasoning",
                    "response_format",
                    "seed",
                    "structured_outputs",
                    "temperature",
                    "tools",
                    "top_p"
                ]
            })
        );
        assert_eq!(
            metadata["capabilities"]["reasoning"],
            json!({
                "supported_efforts": {
                    "kind": "exact",
                    "values": ["high", "low"]
                },
                "default_effort": "high",
                "default_enabled": true,
                "supports_max_tokens": true,
                "mandatory": false
            })
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn assistant_restart_boundaries_preserve_only_durably_safe_checkpoints() {
        let ready_root = tempdir().unwrap();
        let (ready_core, ready_id) = seed_ready_assistant(ready_root.path());
        drop(ready_core);
        let ready_core =
            open_core_after_drop(ready_root.path(), crate::DiscoveryRecoveryOwner::Core);
        assert_eq!(
            ready_core
                .get_provider_discovery_assistant_resume_boundary(&ready_id)
                .unwrap()
                .unwrap()
                .action,
            ProviderDiscoveryAssistantResumeAction::RunAssistant
        );

        let pending_root = tempdir().unwrap();
        let (pending_core, pending_id) = seed_ready_assistant(pending_root.path());
        pending_core
            .provider_discovery()
            .begin_assistant_turn(
                &pending_id,
                AssistantCallEstimate {
                    input_tokens: 16,
                    maximum_output_tokens: 64,
                    maximum_cost_micro_units: 100,
                },
            )
            .unwrap();
        drop(pending_core);
        let pending_core =
            open_core_after_drop(pending_root.path(), crate::DiscoveryRecoveryOwner::Core);
        let pending = pending_core.get_provider_discovery(&pending_id).unwrap();
        assert_eq!(pending.session.state, DiscoveryState::UnknownOutcome);
        assert_eq!(
            pending_core
                .get_provider_discovery_assistant_resume_boundary(&pending_id)
                .unwrap()
                .unwrap()
                .action,
            ProviderDiscoveryAssistantResumeAction::ResolveUnknownOutcome
        );

        let tool_root = tempdir().unwrap();
        let (tool_core, tool_id) = seed_ready_assistant(tool_root.path());
        {
            let orchestrator = tool_core.provider_discovery();
            orchestrator
                .begin_assistant_turn(
                    &tool_id,
                    AssistantCallEstimate {
                        input_tokens: 16,
                        maximum_output_tokens: 64,
                        maximum_cost_micro_units: 100,
                    },
                )
                .unwrap();
            let tool_turn = serde_json::to_vec(&AssistantTurn::CallTool {
                call: AssistantToolCall::ListManifestAdapterFamilies,
            })
            .unwrap();
            assert!(matches!(
                orchestrator
                    .submit_assistant_turn_json(&tool_id, &tool_turn)
                    .unwrap(),
                AssistantHostAction::ExecuteTool { .. }
            ));
        }
        drop(tool_core);
        let tool_core = open_core_after_drop(tool_root.path(), crate::DiscoveryRecoveryOwner::Core);
        assert_eq!(
            tool_core
                .get_provider_discovery_assistant_resume_boundary(&tool_id)
                .unwrap()
                .unwrap()
                .action,
            ProviderDiscoveryAssistantResumeAction::ResumeCoreHostAction
        );
        tool_core
            .resume_provider_discovery_assistant_core_host_action(&tool_id)
            .unwrap();
        assert_eq!(
            tool_core
                .get_provider_discovery_assistant_resume_boundary(&tool_id)
                .unwrap()
                .unwrap()
                .action,
            ProviderDiscoveryAssistantResumeAction::RunAssistant
        );

        let draft_root = tempdir().unwrap();
        let (draft_core, draft_id) = seed_ready_assistant(draft_root.path());
        {
            let orchestrator = draft_core.provider_discovery();
            orchestrator
                .begin_assistant_turn(
                    &draft_id,
                    AssistantCallEstimate {
                        input_tokens: 16,
                        maximum_output_tokens: 256,
                        maximum_cost_micro_units: 100,
                    },
                )
                .unwrap();
            let (manifest, claims) = assistant_manifest_and_claims();
            let evidence_id = EvidenceId::from("assistant-recovery-evidence");
            let mappings = claims
                .iter()
                .map(|claim| FieldEvidenceMapping {
                    field: claim.field().clone(),
                    evidence_ids: vec![evidence_id.clone()],
                    explanation: "The deterministic evidence supports this exact value.".to_owned(),
                })
                .collect::<Vec<_>>();
            let confidence = claims
                .iter()
                .map(|claim| FieldConfidence {
                    field: claim.field().clone(),
                    level: ConfidenceLevel::High,
                    rationale: "Deterministic structural evidence.".to_owned(),
                })
                .collect();
            let turn = AssistantTurn::SubmitDraft {
                draft: Box::new(AssistantManifestDraft {
                    manifest,
                    evidence_mappings: mappings,
                    conflicts: Vec::new(),
                    unresolved_questions: Vec::new(),
                    confidence,
                    summary: "A deterministic evidence-backed provider draft.".to_owned(),
                }),
            };
            assert!(matches!(
                orchestrator
                    .submit_assistant_turn_json(&draft_id, &serde_json::to_vec(&turn).unwrap())
                    .unwrap(),
                AssistantHostAction::ReviewDraft(_)
            ));
        }
        drop(draft_core);
        let draft_core =
            open_core_after_drop(draft_root.path(), crate::DiscoveryRecoveryOwner::Core);
        let boundary = draft_core
            .get_provider_discovery_assistant_resume_boundary(&draft_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            boundary.action,
            ProviderDiscoveryAssistantResumeAction::ReviewDraft
        );
        assert!(boundary.draft_review.is_some());
    }

    struct CredentialReflectingErrorProvider {
        credential: String,
    }

    #[async_trait::async_trait]
    impl Provider for CredentialReflectingErrorProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: false,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            _sink: lorepia_providers::ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            Err(CoreError {
                code: CoreErrorCode::ProviderAuthFailed,
                message: format!("provider reflected {}", self.credential),
                recoverable: false,
                operation_id: format!("operation-{}", self.credential),
            })
        }

        async fn generate_with_internal_plan(
            &self,
            request: GenerationRequest,
            credential: Option<&str>,
            sink: lorepia_providers::ProviderEventSender,
            cancelled: watch::Receiver<bool>,
            _request_plan: lorepia_providers::parameter_mapping::ProviderRequestPlan,
        ) -> CoreResult<GenerationUsage> {
            self.generate(request, credential, sink, cancelled).await
        }
    }

    #[test]
    fn assistant_provider_error_reflection_is_replaced_before_return() {
        let root = tempdir().unwrap();
        let (core, session_id) = seed_ready_assistant(root.path());
        let prompt = core
            .provider_discovery()
            .begin_assistant_turn(
                &session_id,
                AssistantCallEstimate {
                    input_tokens: 16,
                    maximum_output_tokens: 64,
                    maximum_cost_micro_units: 100,
                },
            )
            .unwrap();
        let route = core
            .storage()
            .get_model_route(&ModelRouteId::from("assistant-route"))
            .unwrap();
        let credential = "assistant-error-reflection-canary";
        let error = core
            .runtime_handle()
            .block_on(run_setup_assistant_provider_call(
                Arc::new(CredentialReflectingErrorProvider {
                    credential: credential.to_owned(),
                }),
                &route,
                &prompt,
                AssistantCallEstimate {
                    input_tokens: 16,
                    maximum_output_tokens: 64,
                    maximum_cost_micro_units: 100,
                },
                Some(credential),
            ))
            .unwrap_err();

        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(
            error.message,
            "setup assistant provider error reflected credential material"
        );
        assert!(!format!("{error:?}").contains(credential));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn confirmed_commit_completion_rejects_a_started_wal_without_an_applied_graph() {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(root.path()),
            crate::DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core with native recovery ownership");
        let committing =
            prepare_no_network_credential_commit(&core, "confirmed-completion-operation-authority");
        let first_prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("load first credential install context");
        let first_started = reserve_and_start_credential_install(&core, &first_prepared);
        let interrupted = core
            .attest_provider_discovery_credential_install_no_effect(
                &committing.session.id,
                &first_started.operation_id,
                &first_started.commit_attempt_id,
                &first_started.commit_plan_sha256,
                native_execution_id(&first_started),
            )
            .expect("attest first credential install had no effect");
        assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);

        let restarted = core
            .continue_provider_discovery(
                &committing.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    interrupted.session.revision,
                    ProviderDiscoveryAction::RestartInterrupted,
                )
                .expect("build restart action"),
                None,
            )
            .expect("restart credential commit");
        let retry_prepared = core
            .get_provider_discovery_credential_install_context(&restarted.session.id)
            .expect("load retry credential install context");
        let retry_started = reserve_and_start_credential_install(&core, &retry_prepared);
        assert_ne!(retry_started.operation_id, first_started.operation_id);
        assert_ne!(
            retry_started.native_execution_id, first_started.native_execution_id,
            "unknown-outcome recovery must retain the retry's physical incarnation"
        );
        drop(core);

        let recovered = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
        let unknown = recovered
            .get_provider_discovery(&restarted.session.id)
            .expect("load unknown retry outcome");
        assert_eq!(unknown.session.state, DiscoveryState::UnknownOutcome);
        assert_eq!(
            unknown.session.unknown_operation,
            Some(DiscoveryOperationKind::AtomicCommit)
        );
        assert!(!unknown.session.cancellation_pending);

        let resolution = lorepia_domain::discovery::DiscoveryUnknownOutcomeResolution::
            ConfirmedCommitCompleted {
                connection_id: retry_started.connection_id.clone(),
            };
        // Unknown-outcome proposals are action-specific because the operator
        // must name the exact committed connection. Derive the canonical ID in
        // test code, then exercise the same public action boundary used by a
        // native client.
        let proposal = approval_proposal_for(
            &unknown.session.id,
            unknown.session.revision,
            DiscoveryApprovalGrant::UnknownOutcomeResolution {
                operation: DiscoveryOperationKind::AtomicCommit,
                resolution: resolution.clone(),
            },
        )
        .expect("derive exact confirmed-completion approval");
        let error = recovered
            .continue_provider_discovery(
                &unknown.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    unknown.session.revision,
                    ProviderDiscoveryAction::ResolveUnknownOutcome {
                        approval_id: proposal.id.clone(),
                        resolution,
                    },
                )
                .expect("build confirmed-completion action"),
                None,
            )
            .expect_err("a started WAL alone cannot prove that the graph was committed");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(error.message, "confirmed commit graph is missing");
        assert!(
            recovered
                .list_provider_discovery_approvals(&unknown.session.id)
                .expect("list approvals after rejected resolution")
                .iter()
                .all(|approval| approval.id != proposal.id),
            "a rejected resolution must not retain its approval"
        );
        assert_eq!(
            recovered
                .get_provider_discovery(&unknown.session.id)
                .expect("reload rejected unknown outcome")
                .session
                .state,
            DiscoveryState::UnknownOutcome
        );
        recovered
            .ensure_provider_credential_access_settled(&retry_started.connection_id)
            .expect_err("an uncommitted graph cannot grant credential access");
        drop(recovered);

        let reopened = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
        assert_eq!(
            reopened
                .get_provider_discovery(&unknown.session.id)
                .expect("reload unknown outcome after reopen")
                .session
                .state,
            DiscoveryState::UnknownOutcome
        );
        reopened
            .ensure_provider_credential_access_settled(&retry_started.connection_id)
            .expect_err("reopen must not invent credential ownership");
    }
}
