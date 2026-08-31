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
mod commit;
mod credential;
mod credential_install;
mod deterministic;
mod driver;
mod known_provider;
mod probes;
mod recovery;
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
use commit::commit_plan_for;
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
use recovery::compensation_recipe;
use types::{DiscoverySourceIntent, DiscoveryWorkingDraft};
pub use types::{
    ProviderCurlInspection, ProviderDiscoveryAdditionalEvidence, ProviderDiscoveryCurlInput,
    ProviderDiscoverySource,
};

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

const STANDARD_DISCOVERY_PROBE_PLAN: [CapabilityProbeKind;
    DiscoveryProbeBudget::PROBES_PER_ROUTE as usize] = [
    CapabilityProbeKind::Streaming,
    CapabilityProbeKind::Reasoning,
    CapabilityProbeKind::StructuredOutput,
    CapabilityProbeKind::ToolCalling,
    CapabilityProbeKind::PromptCaching,
];

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

/// Bounded project-owned fixtures for downstream Shell adapter tests.
#[cfg(feature = "test-support")]
#[path = "provider_discovery/test_support.rs"]
pub mod test_support;

#[cfg(test)]
#[path = "provider_discovery/tests/mod.rs"]
mod policy_tests;
