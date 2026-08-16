use std::{net::IpAddr, str::FromStr};

use chrono::{DateTime, Utc};
use lorepia_core::{
    ApiFamily, AssistantDraftReview, AssistantFailureKind, AssistantHostAction,
    AssistantManifestDraft, CanonicalOrigin, ConfidenceLevel, ConflictDisposition, CredentialRef,
    DecoderId, DiscoveryActionId, DiscoveryActionRequired, DiscoveryApprovalBinding,
    DiscoveryApprovalDecision, DiscoveryApprovalGrant, DiscoveryApprovalId,
    DiscoveryApprovalRecord, DiscoveryAssistantCheckpoint, DiscoveryCandidateId,
    DiscoveryCandidateSummary, DiscoveryCommitAttemptId, DiscoveryCommitPhase,
    DiscoveryCompensationKind, DiscoveryCompensationRecord, DiscoveryCompensationStatus,
    DiscoveryEventId, DiscoveryEvidenceKind, DiscoveryEvidenceRecord, DiscoveryFailure,
    DiscoveryInterruptionOutcome, DiscoveryOperationId, DiscoveryOperationKind,
    DiscoveryOperationStatus, DiscoveryOutboxEvent, DiscoveryProgress, DiscoveryProgressPhase,
    DiscoveryRecoveryResult, DiscoveryReviewChange, DiscoveryReviewChangeKind, DiscoveryReviewDiff,
    DiscoverySessionId, DiscoverySessionSnapshot, DiscoveryState,
    DiscoveryUnknownOutcomeResolution, DiscoveryWarning, DraftField, DraftPersistence,
    DraftReviewCheck, EvidenceConflict, EvidenceId, FieldConfidence, FieldEvidenceMapping,
    HttpMethod, HttpUrl, ManifestSourceKind, ModelRouteId, ProviderConnectionId,
    ProviderDiscoveryAction, ProviderDiscoveryAdditionalEvidence,
    ProviderDiscoveryApprovalProposal, ProviderDiscoveryAssistantResumeAction,
    ProviderDiscoveryAssistantResumeBoundary, ProviderDiscoveryConnectionOptions,
    ProviderDiscoveryCredentialAuthority, ProviderDiscoveryCredentialCommitConfirmation,
    ProviderDiscoveryCredentialInstallContext, ProviderDiscoveryCredentialLeaseContext,
    ProviderDiscoveryCurlInput, ProviderDiscoveryReviewProposal, ProviderLocalNetworkApproval,
    ProviderTemplateId, SanitizedDiscoveryInput, StoredDiscoveryCandidate, UnresolvedQuestion,
    provider_discovery_action_envelope,
};
use serde::{Deserialize, Serialize};

use crate::{
    AuthBindingDto, ConnectionConfigEntryDto, ParameterSpecDto, ProviderConnectionDto,
    ProviderCredentialAccessAuthorityContext, ProviderLocalNetworkApprovalDto,
    ProviderLocalNetworkApprovalInput, ProviderNetworkModeInput, RequestPreviewDto,
    SecretCredential, SecretProviderCurl, ShellApi, ShellError, ShellResult, TaskCredentialLease,
    api::validate_identifier,
};

const PROVIDER_DISCOVERY_SNAPSHOT_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoveryConnectionOptionsInput {
    pub values: Vec<ConnectionConfigEntryDto>,
    pub api_base_path: Option<String>,
    pub timeout_seconds: u32,
    pub network_mode: ProviderNetworkModeInput,
    pub local_network_approval: Option<ProviderLocalNetworkApprovalInput>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoveryConnectionOptionsDto {
    pub values: Vec<ConnectionConfigEntryDto>,
    pub api_base_path: Option<String>,
    pub timeout_seconds: u32,
    pub network_mode: String,
    pub local_network_approval: Option<ProviderLocalNetworkApprovalDto>,
}

impl From<ProviderDiscoveryConnectionOptions> for ProviderDiscoveryConnectionOptionsDto {
    fn from(value: ProviderDiscoveryConnectionOptions) -> Self {
        Self {
            values: value.values.into_iter().map(Into::into).collect(),
            api_base_path: value.api_base_path.map(|path| path.as_str().to_owned()),
            timeout_seconds: value.timeout_seconds,
            network_mode: discovery_network_mode_name(value.network_mode).to_owned(),
            local_network_approval: value.local_network_approval.map(Into::into),
        }
    }
}

impl TryFrom<ProviderDiscoveryConnectionOptionsInput> for ProviderDiscoveryConnectionOptions {
    type Error = ShellError;

    fn try_from(value: ProviderDiscoveryConnectionOptionsInput) -> Result<Self, Self::Error> {
        Ok(Self {
            values: value.values.into_iter().map(Into::into).collect(),
            api_base_path: value
                .api_base_path
                .map(|path| {
                    lorepia_core::EndpointPath::parse(&path)
                        .map_err(|_| shell_invalid("discovery API base path is invalid"))
                })
                .transpose()?,
            timeout_seconds: value.timeout_seconds,
            network_mode: value.network_mode.into(),
            local_network_approval: value
                .local_network_approval
                .map(parse_local_network_approval)
                .transpose()?,
            // Issued only by Core at durable session creation; the WebView
            // cannot choose or extend LAN authority lifetime.
            local_network_approved_at: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BeginProviderDiscoverySourceInput {
    Site,
    KnownProvider { template_id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeginProviderDiscoveryInput {
    pub connection_id: String,
    pub display_name: String,
    pub site_url: String,
    pub docs_url: Option<String>,
    /// Requests the native vault slot named by `connection_id`.
    ///
    /// The opaque credential reference itself is never serialized.
    pub credential_binding_requested: bool,
    pub preferred_assistant: Option<String>,
    pub connection_options: ProviderDiscoveryConnectionOptionsInput,
    pub supplied_evidence_ids: Vec<String>,
    pub source: BeginProviderDiscoverySourceInput,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeginProviderDiscoveryCurlInput {
    pub connection_id: String,
    pub display_name: String,
    pub docs_url: Option<String>,
    pub credential_binding_requested: bool,
    pub preferred_assistant: Option<String>,
    pub connection_options: ProviderDiscoveryConnectionOptionsInput,
    pub supplied_evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryFailureDto {
    pub code: String,
    pub message_key: String,
    pub recoverable: bool,
}

impl From<DiscoveryFailure> for DiscoveryFailureDto {
    fn from(value: DiscoveryFailure) -> Self {
        Self {
            code: value.code,
            message_key: value.message_key,
            recoverable: value.recoverable,
        }
    }
}

impl From<DiscoveryFailureDto> for DiscoveryFailure {
    fn from(value: DiscoveryFailureDto) -> Self {
        Self {
            code: value.code,
            message_key: value.message_key,
            recoverable: value.recoverable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryApprovalBindingDto {
    pub approval_id: String,
    pub grant_sha256: String,
}

impl From<DiscoveryApprovalBinding> for DiscoveryApprovalBindingDto {
    fn from(value: DiscoveryApprovalBinding) -> Self {
        Self {
            approval_id: value.approval_id.to_string(),
            grant_sha256: value.grant_sha256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryReviewChangeDto {
    pub kind: String,
    pub target_kind: String,
    pub target_id: String,
    pub summary_key: String,
    pub evidence_ids: Vec<String>,
}

impl From<DiscoveryReviewChange> for DiscoveryReviewChangeDto {
    fn from(value: DiscoveryReviewChange) -> Self {
        Self {
            kind: discovery_review_change_kind_name(value.kind).to_owned(),
            target_kind: value.target_kind,
            target_id: value.target_id,
            summary_key: value.summary_key,
            evidence_ids: value.evidence_ids.into_iter().map(|id| id.0).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryReviewDto {
    pub sha256: String,
    pub graph_sha256: String,
    pub changes: Vec<DiscoveryReviewChangeDto>,
    pub unresolved_question_count: u32,
    pub warning_count: u32,
}

impl From<DiscoveryReviewDiff> for DiscoveryReviewDto {
    fn from(value: DiscoveryReviewDiff) -> Self {
        Self {
            sha256: value.sha256,
            graph_sha256: value.graph_sha256,
            changes: value.changes.into_iter().map(Into::into).collect(),
            unresolved_question_count: value.unresolved_question_count,
            warning_count: value.warning_count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DiscoveryApprovalGrantDto(DiscoveryApprovalGrant);

impl From<DiscoveryApprovalGrant> for DiscoveryApprovalGrantDto {
    fn from(value: DiscoveryApprovalGrant) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoveryApprovalProposalDto {
    pub id: String,
    pub grant: DiscoveryApprovalGrantDto,
    pub grant_sha256: String,
}

impl From<ProviderDiscoveryApprovalProposal> for ProviderDiscoveryApprovalProposalDto {
    fn from(value: ProviderDiscoveryApprovalProposal) -> Self {
        Self {
            id: value.id.to_string(),
            grant: value.grant.into(),
            grant_sha256: value.grant_sha256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoveryReviewProposalDto {
    pub review: DiscoveryReviewDto,
    pub approval: ProviderDiscoveryApprovalProposalDto,
    pub commit_attempt_id: String,
    pub commit_plan_sha256: String,
    pub request_preview: Option<RequestPreviewDto>,
}

impl From<ProviderDiscoveryReviewProposal> for ProviderDiscoveryReviewProposalDto {
    fn from(value: ProviderDiscoveryReviewProposal) -> Self {
        Self {
            review: value.review.into(),
            approval: value.approval.into(),
            commit_attempt_id: value.commit_attempt_id.to_string(),
            commit_plan_sha256: value.commit_plan_sha256,
            request_preview: value.request_preview.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryAssistantFailureKindInput {
    Transport,
    Timeout,
    RateLimited,
    InvalidStructuredOutput,
    DraftRevisionRequired,
    ProviderRejected,
    Internal,
}

impl From<DiscoveryAssistantFailureKindInput> for AssistantFailureKind {
    fn from(value: DiscoveryAssistantFailureKindInput) -> Self {
        match value {
            DiscoveryAssistantFailureKindInput::Transport => Self::Transport,
            DiscoveryAssistantFailureKindInput::Timeout => Self::Timeout,
            DiscoveryAssistantFailureKindInput::RateLimited => Self::RateLimited,
            DiscoveryAssistantFailureKindInput::InvalidStructuredOutput => {
                Self::InvalidStructuredOutput
            }
            DiscoveryAssistantFailureKindInput::DraftRevisionRequired => {
                Self::DraftRevisionRequired
            }
            DiscoveryAssistantFailureKindInput::ProviderRejected => Self::ProviderRejected,
            DiscoveryAssistantFailureKindInput::Internal => Self::Internal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryAssistantInterruptionOutcomeInput {
    ConfirmedNoExternalEffect,
    ExternalOutcomeUnknown,
}

impl From<DiscoveryAssistantInterruptionOutcomeInput> for DiscoveryInterruptionOutcome {
    fn from(value: DiscoveryAssistantInterruptionOutcomeInput) -> Self {
        match value {
            DiscoveryAssistantInterruptionOutcomeInput::ConfirmedNoExternalEffect => {
                Self::ConfirmedNoExternalEffect
            }
            DiscoveryAssistantInterruptionOutcomeInput::ExternalOutcomeUnknown => {
                Self::ExternalOutcomeUnknown
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryAssistantDraftFieldDto {
    ApiFamily,
    DefaultApiOrigin,
    Auth,
    GenerateEndpoint,
    ModelsEndpoint,
    ResponseDecoder,
    StreamingDecoder,
    Parameter { parameter_id: String },
}

impl From<DraftField> for DiscoveryAssistantDraftFieldDto {
    fn from(value: DraftField) -> Self {
        match value {
            DraftField::ApiFamily => Self::ApiFamily,
            DraftField::DefaultApiOrigin => Self::DefaultApiOrigin,
            DraftField::Auth => Self::Auth,
            DraftField::GenerateEndpoint => Self::GenerateEndpoint,
            DraftField::ModelsEndpoint => Self::ModelsEndpoint,
            DraftField::ResponseDecoder => Self::ResponseDecoder,
            DraftField::StreamingDecoder => Self::StreamingDecoder,
            DraftField::Parameter(parameter_id) => Self::Parameter {
                parameter_id: parameter_id.0,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAssistantQuestionDto {
    pub id: String,
    pub field: Option<DiscoveryAssistantDraftFieldDto>,
    pub question: String,
    pub required_evidence: String,
}

impl From<UnresolvedQuestion> for DiscoveryAssistantQuestionDto {
    fn from(value: UnresolvedQuestion) -> Self {
        Self {
            id: value.id,
            field: value.field.map(Into::into),
            question: value.question,
            required_evidence: value.required_evidence,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAssistantEvidenceMappingDto {
    pub field: DiscoveryAssistantDraftFieldDto,
    pub evidence_ids: Vec<String>,
    pub explanation: String,
}

impl From<FieldEvidenceMapping> for DiscoveryAssistantEvidenceMappingDto {
    fn from(value: FieldEvidenceMapping) -> Self {
        Self {
            field: value.field.into(),
            evidence_ids: value.evidence_ids.into_iter().map(|id| id.0).collect(),
            explanation: value.explanation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAssistantFieldConfidenceDto {
    pub field: DiscoveryAssistantDraftFieldDto,
    pub level: String,
    pub rationale: String,
}

impl From<FieldConfidence> for DiscoveryAssistantFieldConfidenceDto {
    fn from(value: FieldConfidence) -> Self {
        Self {
            field: value.field.into(),
            level: assistant_confidence_level_name(value.level).to_owned(),
            rationale: value.rationale,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryAssistantConflictDispositionDto {
    Unresolved,
    Resolved {
        selected_evidence_id: String,
        rationale: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAssistantEvidenceConflictDto {
    pub field: DiscoveryAssistantDraftFieldDto,
    pub evidence_ids: Vec<String>,
    pub disposition: DiscoveryAssistantConflictDispositionDto,
}

impl From<EvidenceConflict> for DiscoveryAssistantEvidenceConflictDto {
    fn from(value: EvidenceConflict) -> Self {
        Self {
            field: value.field.into(),
            evidence_ids: value.evidence_ids.into_iter().map(|id| id.0).collect(),
            disposition: match value.disposition {
                ConflictDisposition::Unresolved => {
                    DiscoveryAssistantConflictDispositionDto::Unresolved
                }
                ConflictDisposition::Resolved {
                    selected_evidence_id,
                    rationale,
                } => DiscoveryAssistantConflictDispositionDto::Resolved {
                    selected_evidence_id: selected_evidence_id.0,
                    rationale,
                },
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAssistantManifestSourceDto {
    pub kind: String,
    pub url: String,
    pub content_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAssistantEndpointDto {
    pub method: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAssistantManifestDto {
    pub schema_version: u32,
    pub api_family: String,
    pub sources: Vec<DiscoveryAssistantManifestSourceDto>,
    pub default_api_origin: Option<String>,
    pub auth: AuthBindingDto,
    pub models_endpoint: Option<DiscoveryAssistantEndpointDto>,
    pub generate_endpoint: DiscoveryAssistantEndpointDto,
    pub response_decoder: String,
    pub streaming_decoder: Option<String>,
    pub parameters: Vec<ParameterSpecDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAssistantManifestDraftDto {
    pub manifest: DiscoveryAssistantManifestDto,
    pub evidence_mappings: Vec<DiscoveryAssistantEvidenceMappingDto>,
    pub conflicts: Vec<DiscoveryAssistantEvidenceConflictDto>,
    pub unresolved_questions: Vec<DiscoveryAssistantQuestionDto>,
    pub confidence: Vec<DiscoveryAssistantFieldConfidenceDto>,
    pub summary: String,
}

impl From<AssistantManifestDraft> for DiscoveryAssistantManifestDraftDto {
    fn from(value: AssistantManifestDraft) -> Self {
        let manifest = value.manifest;
        Self {
            manifest: DiscoveryAssistantManifestDto {
                schema_version: manifest.schema_version,
                api_family: assistant_api_family_name(manifest.api_family).to_owned(),
                sources: manifest
                    .sources
                    .into_iter()
                    .map(|source| DiscoveryAssistantManifestSourceDto {
                        kind: manifest_source_kind_name(source.kind).to_owned(),
                        url: source.url.to_string(),
                        content_sha256: source.content_sha256,
                    })
                    .collect(),
                default_api_origin: manifest.default_api_origin.map(|origin| origin.to_string()),
                auth: AuthBindingDto::from(manifest.auth),
                models_endpoint: manifest.endpoints.models.map(|endpoint| {
                    DiscoveryAssistantEndpointDto {
                        method: assistant_http_method_name(endpoint.method).to_owned(),
                        path: endpoint.path.as_str().to_owned(),
                    }
                }),
                generate_endpoint: DiscoveryAssistantEndpointDto {
                    method: assistant_http_method_name(manifest.endpoints.generate.method)
                        .to_owned(),
                    path: manifest.endpoints.generate.path.as_str().to_owned(),
                },
                response_decoder: assistant_decoder_name(manifest.decoders.response).to_owned(),
                streaming_decoder: manifest
                    .decoders
                    .streaming
                    .map(assistant_decoder_name)
                    .map(str::to_owned),
                parameters: manifest.parameters.into_iter().map(Into::into).collect(),
            },
            evidence_mappings: value
                .evidence_mappings
                .into_iter()
                .map(Into::into)
                .collect(),
            conflicts: value.conflicts.into_iter().map(Into::into).collect(),
            unresolved_questions: value
                .unresolved_questions
                .into_iter()
                .map(Into::into)
                .collect(),
            confidence: value.confidence.into_iter().map(Into::into).collect(),
            summary: value.summary,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAssistantDraftReviewDto {
    pub draft: DiscoveryAssistantManifestDraftDto,
    pub unresolved_conflicts: Vec<DiscoveryAssistantDraftFieldDto>,
    pub required_checks: Vec<String>,
    pub persistence: String,
}

impl From<AssistantDraftReview> for DiscoveryAssistantDraftReviewDto {
    fn from(value: AssistantDraftReview) -> Self {
        Self {
            draft: value.draft.into(),
            unresolved_conflicts: value
                .unresolved_conflicts
                .into_iter()
                .map(Into::into)
                .collect(),
            required_checks: value
                .requirements
                .required_checks
                .into_iter()
                .map(assistant_review_check_name)
                .map(str::to_owned)
                .collect(),
            persistence: assistant_persistence_name(value.requirements.persistence).to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryAssistantHostActionDto {
    RequestMoreEvidence {
        session_id: String,
        questions: Vec<DiscoveryAssistantQuestionDto>,
    },
    ReviewDraft {
        review: Box<DiscoveryAssistantDraftReviewDto>,
    },
}

impl TryFrom<AssistantHostAction> for DiscoveryAssistantHostActionDto {
    type Error = ShellError;

    fn try_from(value: AssistantHostAction) -> Result<Self, Self::Error> {
        match value {
            AssistantHostAction::ExecuteTool { .. } => Err(lorepia_core::CoreError::internal(
                "setup assistant tool execution escaped the Core-owned tool loop",
            )
            .into()),
            AssistantHostAction::RequestMoreEvidence {
                session_id,
                questions,
            } => Ok(Self::RequestMoreEvidence {
                session_id: session_id.0,
                questions: questions.into_iter().map(Into::into).collect(),
            }),
            AssistantHostAction::ReviewDraft(review) => Ok(Self::ReviewDraft {
                review: Box::new((*review).into()),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAssistantResumeBoundaryDto {
    pub checkpoint: Option<String>,
    pub action: String,
    pub questions: Vec<DiscoveryAssistantQuestionDto>,
    pub draft_review: Option<DiscoveryAssistantDraftReviewDto>,
}

impl From<ProviderDiscoveryAssistantResumeBoundary> for DiscoveryAssistantResumeBoundaryDto {
    fn from(value: ProviderDiscoveryAssistantResumeBoundary) -> Self {
        Self {
            checkpoint: value
                .checkpoint
                .map(assistant_checkpoint_name)
                .map(str::to_owned),
            action: assistant_resume_action_name(value.action).to_owned(),
            questions: value.questions.into_iter().map(Into::into).collect(),
            draft_review: value.draft_review.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryStepDto {
    pub id: String,
    pub title_key: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoverySessionDto {
    pub snapshot_schema_version: u32,
    pub id: String,
    pub connection_id: String,
    pub display_name: String,
    pub site_url: String,
    pub docs_url: Option<String>,
    pub credential_binding_requested: bool,
    pub preferred_assistant: Option<String>,
    pub connection_options: ProviderDiscoveryConnectionOptionsDto,
    pub supplied_evidence_ids: Vec<String>,
    pub state: String,
    pub revision: u64,
    pub next_event_sequence: u64,
    pub steps: Vec<DiscoveryStepDto>,
    pub action_required: Option<DiscoveryActionRequiredDto>,
    pub active_operation_id: Option<String>,
    pub recovery_operation: Option<String>,
    pub unknown_operation: Option<String>,
    pub manifest_sha256: Option<String>,
    pub commit_plan_sha256: Option<String>,
    pub commit_attempt_id: Option<String>,
    pub committed_connection_id: Option<String>,
    pub cancellation_pending: bool,
    pub active_effect_approval: Option<DiscoveryApprovalBindingDto>,
    pub failure: Option<DiscoveryFailureDto>,
    /// Whether Core retains a private redacted working draft.
    ///
    /// The draft, assistant prompt, response, and tool payload never cross IPC.
    pub has_private_draft: bool,
    pub review: Option<DiscoveryReviewDto>,
    pub assistant_resume_boundary: Option<DiscoveryAssistantResumeBoundaryDto>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProviderDiscoverySessionDto {
    fn from_snapshot(
        value: DiscoverySessionSnapshot,
        assistant_resume_boundary: Option<ProviderDiscoveryAssistantResumeBoundary>,
    ) -> Self {
        let state = value.session.state;
        let steps = discovery_steps(state);
        let action_required = action_required_for_snapshot(&value);
        let input = value.session.input;
        Self {
            snapshot_schema_version: PROVIDER_DISCOVERY_SNAPSHOT_SCHEMA_VERSION,
            id: value.session.id.0,
            connection_id: input.connection_id.0,
            display_name: input.display_name,
            site_url: input.site_url.to_string(),
            docs_url: input.docs_url.map(|url| url.to_string()),
            credential_binding_requested: input.credential_ref.is_some(),
            preferred_assistant: input.preferred_assistant.map(|id| id.0),
            connection_options: input.connection_options.into(),
            supplied_evidence_ids: input
                .supplied_evidence_ids
                .into_iter()
                .map(|id| id.0)
                .collect(),
            state: discovery_state_name(state).to_owned(),
            revision: value.session.revision,
            next_event_sequence: value.session.next_event_sequence,
            steps,
            action_required,
            active_operation_id: value.active_operation_id.map(|id| id.to_string()),
            recovery_operation: value
                .session
                .recovery
                .map(|checkpoint| discovery_operation_name(checkpoint.operation).to_owned()),
            unknown_operation: value
                .session
                .unknown_operation
                .map(discovery_operation_name)
                .map(str::to_owned),
            manifest_sha256: value.session.manifest_sha256,
            commit_plan_sha256: value.session.commit_plan_sha256,
            commit_attempt_id: value.session.commit_attempt_id.map(|id| id.to_string()),
            committed_connection_id: value.session.committed_connection_id.map(|id| id.0),
            cancellation_pending: value.session.cancellation_pending,
            active_effect_approval: value.session.active_effect_approval.map(Into::into),
            failure: value.session.failure.map(Into::into),
            has_private_draft: value.draft_json.is_some(),
            review: value.review.map(Into::into),
            assistant_resume_boundary: assistant_resume_boundary.map(Into::into),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryCandidateSummaryDto {
    ProviderTemplate {
        template_id: String,
        template_version: u32,
    },
    ApiOrigin {
        origin: String,
    },
    OfficialDocument {
        url: String,
        content_sha256: String,
    },
    ModelRoute {
        model_id: String,
    },
    ManifestDraft {
        schema_version: u32,
        manifest_sha256: String,
    },
}

impl From<DiscoveryCandidateSummary> for DiscoveryCandidateSummaryDto {
    fn from(value: DiscoveryCandidateSummary) -> Self {
        match value {
            DiscoveryCandidateSummary::ProviderTemplate {
                template_id,
                template_version,
            } => Self::ProviderTemplate {
                template_id: template_id.0,
                template_version,
            },
            DiscoveryCandidateSummary::ApiOrigin { origin } => Self::ApiOrigin {
                origin: origin.to_string(),
            },
            DiscoveryCandidateSummary::OfficialDocument {
                url,
                content_sha256,
            } => Self::OfficialDocument {
                url: url.to_string(),
                content_sha256,
            },
            DiscoveryCandidateSummary::ModelRoute { model_id } => Self::ModelRoute { model_id },
            DiscoveryCandidateSummary::ManifestDraft {
                schema_version,
                manifest_sha256,
            } => Self::ManifestDraft {
                schema_version,
                manifest_sha256,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryCandidateDto {
    pub id: String,
    pub session_id: String,
    pub summary: DiscoveryCandidateSummaryDto,
    pub evidence_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub proposed_revision: u64,
}

impl From<StoredDiscoveryCandidate> for DiscoveryCandidateDto {
    fn from(value: StoredDiscoveryCandidate) -> Self {
        Self {
            id: value.candidate.id.to_string(),
            session_id: value.candidate.session_id.0,
            summary: value.candidate.summary.into(),
            evidence_ids: value
                .candidate
                .evidence_ids
                .into_iter()
                .map(|id| id.0)
                .collect(),
            created_at: value.candidate.created_at,
            proposed_revision: value.proposed_revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryEvidenceDto {
    pub id: String,
    pub session_id: String,
    pub kind: String,
    pub source_url: String,
    pub content_sha256: String,
    pub fetched_at: DateTime<Utc>,
}

impl From<DiscoveryEvidenceRecord> for DiscoveryEvidenceDto {
    fn from(value: DiscoveryEvidenceRecord) -> Self {
        Self {
            id: value.id.0,
            session_id: value.session_id.0,
            kind: discovery_evidence_kind_name(value.kind).to_owned(),
            source_url: value.source_url.to_string(),
            content_sha256: value.content_sha256,
            fetched_at: value.fetched_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryApprovalRecordDto {
    pub id: String,
    pub session_id: String,
    pub session_revision: u64,
    pub decision: String,
    pub grant: DiscoveryApprovalGrantDto,
    pub created_at: DateTime<Utc>,
}

impl From<DiscoveryApprovalRecord> for DiscoveryApprovalRecordDto {
    fn from(value: DiscoveryApprovalRecord) -> Self {
        Self {
            id: value.id.to_string(),
            session_id: value.session_id.0,
            session_revision: value.session_revision,
            decision: match value.decision {
                DiscoveryApprovalDecision::Approved => "approved",
                DiscoveryApprovalDecision::Rejected => "rejected",
            }
            .to_owned(),
            grant: value.grant.into(),
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resolution", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryUnknownOutcomeResolutionInput {
    ConfirmedNoEffect,
    ConfirmedCommitCompleted { connection_id: String },
    ConfirmedCompensated,
    ManuallyReconciledAsFailed,
}

impl TryFrom<DiscoveryUnknownOutcomeResolutionInput> for DiscoveryUnknownOutcomeResolution {
    type Error = ShellError;

    fn try_from(value: DiscoveryUnknownOutcomeResolutionInput) -> Result<Self, Self::Error> {
        Ok(match value {
            DiscoveryUnknownOutcomeResolutionInput::ConfirmedNoEffect => Self::ConfirmedNoEffect,
            DiscoveryUnknownOutcomeResolutionInput::ConfirmedCommitCompleted { connection_id } => {
                validate_identifier("connection_id", &connection_id)?;
                Self::ConfirmedCommitCompleted {
                    connection_id: ProviderConnectionId::from(connection_id),
                }
            }
            DiscoveryUnknownOutcomeResolutionInput::ConfirmedCompensated => {
                Self::ConfirmedCompensated
            }
            DiscoveryUnknownOutcomeResolutionInput::ManuallyReconciledAsFailed => {
                Self::ManuallyReconciledAsFailed
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContinueProviderDiscoveryActionInput {
    SelectTemplate {
        candidate_id: String,
    },
    ContinueWithoutTemplate,
    SupplyMoreEvidence {
        evidence_ids: Vec<String>,
    },
    RequestAssistant,
    ApproveAssistant {
        approval_id: String,
        approval_grant_sha256: String,
    },
    DeclineAssistant,
    ApproveCredentialOrigin {
        approval_id: String,
    },
    ApproveProbes {
        approval_id: String,
        approval_grant_sha256: String,
    },
    SkipProbes,
    ApproveReview {
        approval_id: String,
        commit_attempt_id: String,
        commit_plan_sha256: String,
        graph_sha256: String,
    },
    ResumeCompensation,
    RestartInterrupted,
    ResolveUnknownOutcome {
        approval_id: String,
        resolution: DiscoveryUnknownOutcomeResolutionInput,
    },
}

impl TryFrom<ContinueProviderDiscoveryActionInput> for ProviderDiscoveryAction {
    type Error = ShellError;

    fn try_from(value: ContinueProviderDiscoveryActionInput) -> Result<Self, Self::Error> {
        Ok(match value {
            ContinueProviderDiscoveryActionInput::SelectTemplate { candidate_id } => {
                Self::SelectTemplate {
                    candidate_id: DiscoveryCandidateId::parse(candidate_id)
                        .map_err(|_| shell_invalid("candidate_id is invalid"))?,
                }
            }
            ContinueProviderDiscoveryActionInput::ContinueWithoutTemplate => {
                Self::ContinueWithoutTemplate
            }
            ContinueProviderDiscoveryActionInput::SupplyMoreEvidence { evidence_ids } => {
                Self::SupplyMoreEvidence {
                    evidence_ids: parse_evidence_ids(evidence_ids)?,
                }
            }
            ContinueProviderDiscoveryActionInput::RequestAssistant => Self::RequestAssistant,
            ContinueProviderDiscoveryActionInput::ApproveAssistant {
                approval_id,
                approval_grant_sha256,
            } => {
                validate_sha256("approval_grant_sha256", &approval_grant_sha256)?;
                Self::ApproveAssistant {
                    approval_id: parse_approval_id(&approval_id)?,
                    approval_grant_sha256,
                }
            }
            ContinueProviderDiscoveryActionInput::DeclineAssistant => Self::DeclineAssistant,
            ContinueProviderDiscoveryActionInput::ApproveCredentialOrigin { approval_id } => {
                Self::ApproveCredentialOrigin {
                    approval_id: parse_approval_id(&approval_id)?,
                }
            }
            ContinueProviderDiscoveryActionInput::ApproveProbes {
                approval_id,
                approval_grant_sha256,
            } => {
                validate_sha256("approval_grant_sha256", &approval_grant_sha256)?;
                Self::ApproveProbes {
                    approval_id: parse_approval_id(&approval_id)?,
                    approval_grant_sha256,
                }
            }
            ContinueProviderDiscoveryActionInput::SkipProbes => Self::SkipProbes,
            ContinueProviderDiscoveryActionInput::ApproveReview {
                approval_id,
                commit_attempt_id,
                commit_plan_sha256,
                graph_sha256,
            } => {
                validate_sha256("commit_plan_sha256", &commit_plan_sha256)?;
                validate_sha256("graph_sha256", &graph_sha256)?;
                Self::ApproveReview {
                    approval_id: parse_approval_id(&approval_id)?,
                    commit_attempt_id: DiscoveryCommitAttemptId::parse(commit_attempt_id)
                        .map_err(|_| shell_invalid("commit_attempt_id is invalid"))?,
                    commit_plan_sha256,
                    graph_sha256,
                }
            }
            ContinueProviderDiscoveryActionInput::ResumeCompensation => Self::ResumeCompensation,
            ContinueProviderDiscoveryActionInput::RestartInterrupted => Self::RestartInterrupted,
            ContinueProviderDiscoveryActionInput::ResolveUnknownOutcome {
                approval_id,
                resolution,
            } => Self::ResolveUnknownOutcome {
                approval_id: parse_approval_id(&approval_id)?,
                resolution: resolution.try_into()?,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinueProviderDiscoveryInput {
    pub session_id: String,
    pub action_id: String,
    pub expected_revision: u64,
    pub action: ContinueProviderDiscoveryActionInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryProgressDto {
    pub phase: String,
    pub completed: u32,
    pub total: Option<u32>,
}

impl From<DiscoveryProgress> for DiscoveryProgressDto {
    fn from(value: DiscoveryProgress) -> Self {
        Self {
            phase: match value.phase {
                DiscoveryProgressPhase::ProviderCandidates => "provider_candidates",
                DiscoveryProgressPhase::Documents => "documents",
                DiscoveryProgressPhase::Evidence => "evidence",
                DiscoveryProgressPhase::Models => "models",
                DiscoveryProgressPhase::Probes => "probes",
            }
            .to_owned(),
            completed: value.completed,
            total: value.total,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryActionRequiredDto {
    pub kind: String,
    pub operation: Option<String>,
}

impl From<DiscoveryActionRequired> for DiscoveryActionRequiredDto {
    fn from(value: DiscoveryActionRequired) -> Self {
        match value {
            DiscoveryActionRequired::SelectTemplate => Self::simple("select_template"),
            DiscoveryActionRequired::SupplyMoreEvidence => Self::simple("supply_more_evidence"),
            DiscoveryActionRequired::ApproveAssistant => Self::simple("approve_assistant"),
            DiscoveryActionRequired::ApproveCredentialOrigin => {
                Self::simple("approve_credential_origin")
            }
            DiscoveryActionRequired::ApproveProbes => Self::simple("approve_probes"),
            DiscoveryActionRequired::Review => Self::simple("review"),
            DiscoveryActionRequired::RestartInterrupted { operation } => Self {
                kind: "restart_interrupted".to_owned(),
                operation: Some(discovery_operation_name(operation).to_owned()),
            },
            DiscoveryActionRequired::ReconcileUnknownOutcome { operation } => Self {
                kind: "reconcile_unknown_outcome".to_owned(),
                operation: Some(discovery_operation_name(operation).to_owned()),
            },
        }
    }
}

impl DiscoveryActionRequiredDto {
    fn simple(kind: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            operation: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoveryEventDto {
    pub version: u32,
    pub id: String,
    pub session_id: String,
    pub sequence: u64,
    pub session_revision: u64,
    pub state: String,
    pub progress: Option<DiscoveryProgressDto>,
    pub action_required: Option<DiscoveryActionRequiredDto>,
    pub warning: Option<String>,
    pub action_id: String,
    pub failure: Option<DiscoveryFailureDto>,
}

impl From<lorepia_core::ProviderDiscoveryEvent> for ProviderDiscoveryEventDto {
    fn from(value: lorepia_core::ProviderDiscoveryEvent) -> Self {
        Self {
            version: value.version,
            id: value.id.to_string(),
            session_id: value.session_id.0,
            sequence: value.sequence,
            session_revision: value.session_revision,
            state: discovery_state_name(value.state).to_owned(),
            progress: value.progress.map(Into::into),
            action_required: value.action_required.map(Into::into),
            warning: value.warning.map(discovery_warning_name).map(str::to_owned),
            action_id: value.action_id.to_string(),
            failure: value.failure.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryOutboxEventDto {
    pub event: ProviderDiscoveryEventDto,
    pub delivery_attempts: u32,
    pub available_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl From<DiscoveryOutboxEvent> for DiscoveryOutboxEventDto {
    fn from(value: DiscoveryOutboxEvent) -> Self {
        Self {
            event: value.event.into(),
            delivery_attempts: value.delivery_attempts,
            available_at: value.available_at,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryRecoveryResultDto {
    pub operation_id: String,
    pub session_id: String,
    pub state: String,
    pub event: ProviderDiscoveryEventDto,
}

/// Backend-only projection used to bind a native vault write to one WAL
/// attempt. It deliberately has no serde implementation and is never an IPC
/// response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCredentialInstallContextDto {
    pub session_id: String,
    pub session_revision: u64,
    pub operation_id: String,
    pub operation_status: String,
    /// Durable slot reservation available while the operation is still
    /// Prepared. It is not authority to publish or compensate a credential.
    pub native_execution_reservation_id: Option<String>,
    /// Physical native-slot incarnation. Prepared operations deliberately
    /// expose no usable authority; Core activates the exact reservation with
    /// the durable Started transition.
    pub native_execution_id: Option<String>,
    pub commit_attempt_id: String,
    pub commit_plan_sha256: String,
    pub commit_phase: String,
    pub connection_id: String,
    pub connection_binding_sha256: String,
}

/// Backend-only proof of the exact native slot observed for a discovery
/// commit. This type deliberately has no serde implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCredentialCommitConfirmationDto {
    pub operation_id: String,
    pub native_execution_id: String,
    pub commit_attempt_id: String,
    pub commit_plan_sha256: String,
    pub connection_id: String,
    pub connection_binding_sha256: String,
}

impl TryFrom<&ProviderDiscoveryCredentialInstallContextDto>
    for ProviderDiscoveryCredentialCommitConfirmationDto
{
    type Error = ShellError;

    fn try_from(value: &ProviderDiscoveryCredentialInstallContextDto) -> ShellResult<Self> {
        if value.operation_status != "started"
            || value.native_execution_reservation_id.as_deref()
                != value.native_execution_id.as_deref()
        {
            return Err(shell_invalid(
                "credential confirmation requires the exact started reservation",
            ));
        }
        let native_execution_id = value.native_execution_id.clone().ok_or_else(|| {
            shell_invalid("credential confirmation requires a started native execution")
        })?;
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

/// Backend-only projection used for a discovery-scoped native credential.
///
/// This type deliberately has no serde implementation and cannot cross the
/// frontend IPC boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCredentialLeaseContextDto {
    pub session_id: String,
    pub connection_id: String,
    pub credential_api_origin: String,
    pub credential_origin_approval_id: String,
    pub credential_origin_grant_sha256: String,
    pub connection_binding_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCredentialAuthorityDto {
    pub operation_id: String,
    pub native_execution_id: String,
    pub commit_attempt_id: String,
    pub connection_id: String,
    pub credential_api_origin: String,
    pub credential_origin_approval_id: String,
    pub credential_origin_grant_sha256: String,
    pub connection_binding_sha256: String,
}

impl From<ProviderDiscoveryCredentialAuthority> for ProviderDiscoveryCredentialAuthorityDto {
    fn from(value: ProviderDiscoveryCredentialAuthority) -> Self {
        Self {
            operation_id: value.operation_id.to_string(),
            native_execution_id: value.native_execution_id,
            commit_attempt_id: value.commit_attempt_id.to_string(),
            connection_id: value.connection_id.0,
            credential_api_origin: value.credential_api_origin.to_string(),
            credential_origin_approval_id: value.credential_origin_approval_id.to_string(),
            credential_origin_grant_sha256: value.credential_origin_grant_sha256,
            connection_binding_sha256: value.connection_binding_sha256,
        }
    }
}

impl From<ProviderDiscoveryCredentialInstallContext>
    for ProviderDiscoveryCredentialInstallContextDto
{
    fn from(value: ProviderDiscoveryCredentialInstallContext) -> Self {
        Self {
            session_id: value.session_id.0,
            session_revision: value.session_revision,
            operation_id: value.operation_id.to_string(),
            operation_status: discovery_operation_status_name(value.operation_status).to_owned(),
            native_execution_reservation_id: value.native_execution_reservation_id,
            native_execution_id: value.native_execution_id,
            commit_attempt_id: value.commit_attempt_id.to_string(),
            commit_plan_sha256: value.commit_plan_sha256,
            commit_phase: discovery_commit_phase_name(value.commit_phase).to_owned(),
            connection_id: value.connection_id.0,
            connection_binding_sha256: value.connection_binding_sha256,
        }
    }
}

impl From<ProviderDiscoveryCredentialLeaseContext> for ProviderDiscoveryCredentialLeaseContextDto {
    fn from(value: ProviderDiscoveryCredentialLeaseContext) -> Self {
        Self {
            session_id: value.session_id.0,
            connection_id: value.connection_id.0,
            credential_api_origin: value.credential_api_origin.to_string(),
            credential_origin_approval_id: value.credential_origin_approval_id.to_string(),
            credential_origin_grant_sha256: value.credential_origin_grant_sha256,
            connection_binding_sha256: value.connection_binding_sha256,
        }
    }
}

impl From<DiscoveryRecoveryResult> for DiscoveryRecoveryResultDto {
    fn from(value: DiscoveryRecoveryResult) -> Self {
        Self {
            operation_id: value.operation_id.to_string(),
            session_id: value.session_id.0,
            state: discovery_state_name(value.state).to_owned(),
            event: value.event.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryCompensationRecordDto {
    pub id: String,
    pub commit_attempt_id: String,
    pub ordinal: u32,
    pub action_id: String,
    pub kind: String,
    pub status: String,
    pub attempt_count: u32,
    pub last_failure: Option<DiscoveryFailureDto>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl From<DiscoveryCompensationRecord> for DiscoveryCompensationRecordDto {
    fn from(value: DiscoveryCompensationRecord) -> Self {
        Self {
            id: value.id,
            commit_attempt_id: value.commit_attempt_id.to_string(),
            ordinal: value.ordinal,
            action_id: value.action_id.to_string(),
            kind: discovery_compensation_kind_name(value.kind).to_owned(),
            status: discovery_compensation_status_name(value.status).to_owned(),
            attempt_count: value.attempt_count,
            last_failure: value.last_failure.map(Into::into),
            created_at: value.created_at,
            updated_at: value.updated_at,
            completed_at: value.completed_at,
        }
    }
}

impl ShellApi {
    fn project_provider_discovery_session(
        &self,
        snapshot: DiscoverySessionSnapshot,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let session_id = snapshot.session.id.clone();
        let assistant_resume_boundary = self
            .core
            .get_provider_discovery_assistant_resume_boundary(&session_id)
            .map_err(ShellError::from)?;
        Ok(ProviderDiscoverySessionDto::from_snapshot(
            snapshot,
            assistant_resume_boundary,
        ))
    }

    pub fn begin_provider_discovery(
        &self,
        input: BeginProviderDiscoveryInput,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        self.begin_provider_discovery_with_credential_authority(input, None)
    }

    /// Backend-only admission boundary carrying the exact durable authority
    /// used for a native credential read. The authority is never serialized
    /// to the webview.
    pub fn begin_provider_discovery_with_credential_authority(
        &self,
        input: BeginProviderDiscoveryInput,
        credential_authority: Option<ProviderCredentialAccessAuthorityContext>,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let source = input.source.clone();
        let sanitized = sanitized_discovery_input(
            input.connection_id,
            input.display_name,
            input.site_url,
            input.docs_url,
            input.credential_binding_requested,
            input.preferred_assistant,
            input.connection_options,
            input.supplied_evidence_ids,
        )?;
        let snapshot = match source {
            BeginProviderDiscoverySourceInput::Site => self
                .core
                .begin_provider_discovery_site_with_credential_authority(
                    sanitized,
                    credential_authority.map(core_credential_access_authority),
                ),
            BeginProviderDiscoverySourceInput::KnownProvider { template_id } => {
                validate_identifier("template_id", &template_id)?;
                self.core
                    .begin_provider_discovery_known_with_credential_authority(
                        sanitized,
                        ProviderTemplateId::from(template_id),
                        credential_authority.map(core_credential_access_authority),
                    )
            }
        }
        .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn begin_provider_discovery_curl(
        &self,
        input: BeginProviderDiscoveryCurlInput,
        curl: SecretProviderCurl,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        self.begin_provider_discovery_curl_with_credential_authority(input, curl, None)
    }

    /// Backend-only cURL admission boundary carrying the exact durable
    /// credential-read authority for an existing connection identifier.
    pub fn begin_provider_discovery_curl_with_credential_authority(
        &self,
        input: BeginProviderDiscoveryCurlInput,
        curl: SecretProviderCurl,
        credential_authority: Option<ProviderCredentialAccessAuthorityContext>,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        validate_identifier("connection_id", &input.connection_id)?;
        let connection_id = ProviderConnectionId::from(input.connection_id);
        let curl_input = ProviderDiscoveryCurlInput {
            credential_ref: input
                .credential_binding_requested
                .then(|| CredentialRef(connection_id.as_str().to_owned())),
            connection_id,
            display_name: input.display_name,
            docs_url: input
                .docs_url
                .map(|url| parse_http_url("docs_url", &url))
                .transpose()?,
            preferred_assistant: input
                .preferred_assistant
                .map(|id| {
                    validate_identifier("preferred_assistant", &id)?;
                    Ok::<_, ShellError>(ModelRouteId::from(id))
                })
                .transpose()?,
            connection_options: input.connection_options.try_into()?,
            supplied_evidence_ids: parse_evidence_ids(input.supplied_evidence_ids)?,
        };
        let snapshot = self
            .core
            .begin_provider_discovery_curl_with_credential_authority(
                curl_input,
                curl.into_core_value(),
                credential_authority.map(core_credential_access_authority),
            )
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn list_provider_discoveries(
        &self,
        limit: u32,
    ) -> ShellResult<Vec<ProviderDiscoverySessionDto>> {
        let snapshots = self
            .core
            .list_provider_discoveries(limit)
            .map_err(ShellError::from)?;
        snapshots
            .into_iter()
            .map(|snapshot| self.project_provider_discovery_session(snapshot))
            .collect()
    }

    /// Backend-only complete startup-recovery scan.
    pub fn list_unfinished_provider_discovery_recovery_candidates(
        &self,
    ) -> ShellResult<Vec<ProviderDiscoverySessionDto>> {
        let snapshots = self
            .core
            .list_unfinished_provider_discovery_recovery_candidates()
            .map_err(ShellError::from)?;
        snapshots
            .into_iter()
            .map(|snapshot| self.project_provider_discovery_session(snapshot))
            .collect()
    }

    /// Backend-only complete scan of credential WALs that must be reconciled
    /// with the native vault before generic recovery runs.
    pub fn list_provider_discovery_credential_recovery_candidates(
        &self,
    ) -> ShellResult<Vec<ProviderDiscoverySessionDto>> {
        let snapshots = self
            .core
            .list_provider_discovery_credential_recovery_candidates()
            .map_err(ShellError::from)?;
        snapshots
            .into_iter()
            .map(|snapshot| self.project_provider_discovery_session(snapshot))
            .collect()
    }

    pub fn get_provider_discovery(
        &self,
        session_id: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .get_provider_discovery(&parse_session_id(session_id)?)
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn list_provider_discovery_candidates(
        &self,
        session_id: &str,
    ) -> ShellResult<Vec<DiscoveryCandidateDto>> {
        self.core
            .list_provider_discovery_candidates(&parse_session_id(session_id)?)
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn list_provider_discovery_evidence(
        &self,
        session_id: &str,
    ) -> ShellResult<Vec<DiscoveryEvidenceDto>> {
        self.core
            .list_provider_discovery_evidence(&parse_session_id(session_id)?)
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn list_provider_discovery_approvals(
        &self,
        session_id: &str,
    ) -> ShellResult<Vec<DiscoveryApprovalRecordDto>> {
        self.core
            .list_provider_discovery_approvals(&parse_session_id(session_id)?)
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn get_provider_discovery_review(
        &self,
        session_id: &str,
    ) -> ShellResult<Option<DiscoveryReviewDto>> {
        self.core
            .get_provider_discovery_review(&parse_session_id(session_id)?)
            .map(|review| review.map(Into::into))
            .map_err(ShellError::from)
    }

    pub fn get_provider_discovery_approval_proposal(
        &self,
        session_id: &str,
    ) -> ShellResult<Option<ProviderDiscoveryApprovalProposalDto>> {
        self.core
            .get_provider_discovery_approval_proposal(&parse_session_id(session_id)?)
            .map(|proposal| proposal.map(Into::into))
            .map_err(ShellError::from)
    }

    pub fn get_provider_discovery_review_proposal(
        &self,
        session_id: &str,
    ) -> ShellResult<Option<ProviderDiscoveryReviewProposalDto>> {
        self.core
            .get_provider_discovery_review_proposal(&parse_session_id(session_id)?)
            .map(|proposal| proposal.map(Into::into))
            .map_err(ShellError::from)
    }

    pub fn get_provider_discovery_assistant_resume_boundary(
        &self,
        session_id: &str,
    ) -> ShellResult<Option<DiscoveryAssistantResumeBoundaryDto>> {
        self.core
            .get_provider_discovery_assistant_resume_boundary(&parse_session_id(session_id)?)
            .map(|boundary| boundary.map(Into::into))
            .map_err(ShellError::from)
    }

    pub fn resume_provider_discovery_assistant_core_host_action(
        &self,
        session_id: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .resume_provider_discovery_assistant_core_host_action(&parse_session_id(session_id)?)
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn approve_provider_discovery_assistant_retry(
        &self,
        session_id: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .approve_provider_discovery_assistant_retry(&parse_session_id(session_id)?)
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn request_provider_discovery_assistant_revision(
        &self,
        session_id: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .request_provider_discovery_assistant_revision(&parse_session_id(session_id)?)
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn accept_provider_discovery_assistant_draft(
        &self,
        session_id: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .accept_provider_discovery_assistant_draft(&parse_session_id(session_id)?)
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn record_provider_discovery_assistant_failure(
        &self,
        session_id: &str,
        kind: DiscoveryAssistantFailureKindInput,
        retryable: bool,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .record_provider_discovery_assistant_failure(
                &parse_session_id(session_id)?,
                kind.into(),
                retryable,
            )
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn interrupt_provider_discovery_assistant(
        &self,
        session_id: &str,
        outcome: DiscoveryAssistantInterruptionOutcomeInput,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .interrupt_provider_discovery_assistant(&parse_session_id(session_id)?, outcome.into())
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn restart_provider_discovery_assistant_after_interruption(
        &self,
        session_id: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .restart_provider_discovery_assistant_after_interruption(&parse_session_id(session_id)?)
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn continue_provider_discovery(
        &self,
        input: ContinueProviderDiscoveryInput,
        credential: Option<SecretCredential>,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let (_cancel, cancelled) = tokio::sync::watch::channel(false);
        self.continue_provider_discovery_inner(input, credential, cancelled)
    }

    pub fn continue_provider_discovery_with_credential_dispatch(
        &self,
        input: ContinueProviderDiscoveryInput,
        credential: SecretCredential,
        dispatch_lease: TaskCredentialLease,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let result = self.continue_provider_discovery_inner(input, Some(credential), cancelled);
        drop(dispatch_lease);
        result
    }

    fn continue_provider_discovery_inner(
        &self,
        input: ContinueProviderDiscoveryInput,
        credential: Option<SecretCredential>,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let session_id = parse_session_id(&input.session_id)?;
        let action_id = DiscoveryActionId::parse(input.action_id)
            .map_err(|_| shell_invalid("action_id is invalid"))?;
        let envelope = provider_discovery_action_envelope(
            action_id,
            input.expected_revision,
            input.action.try_into()?,
        )
        .map_err(ShellError::from)?;
        let snapshot = self
            .core
            .continue_provider_discovery_with_cancellation(
                &session_id,
                envelope,
                credential.as_ref().map(SecretCredential::expose_to_core),
                cancelled,
            )
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn supply_provider_discovery_document_evidence(
        &self,
        session_id: &str,
        expected_revision: u64,
        document_url: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .supply_provider_discovery_evidence(
                &parse_session_id(session_id)?,
                expected_revision,
                ProviderDiscoveryAdditionalEvidence::document_url(parse_http_url(
                    "document_url",
                    document_url,
                )?),
            )
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn supply_provider_discovery_curl_evidence(
        &self,
        session_id: &str,
        expected_revision: u64,
        curl: SecretProviderCurl,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .supply_provider_discovery_evidence(
                &parse_session_id(session_id)?,
                expected_revision,
                ProviderDiscoveryAdditionalEvidence::curl(curl.into_core_value()),
            )
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn cancel_provider_discovery(
        &self,
        session_id: &str,
        expected_revision: u64,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .cancel_provider_discovery(&parse_session_id(session_id)?, expected_revision)
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn commit_provider_discovery(
        &self,
        session_id: &str,
        credential_confirmation: Option<&ProviderDiscoveryCredentialCommitConfirmationDto>,
    ) -> ShellResult<ProviderConnectionDto> {
        let credential_confirmation = credential_confirmation
            .map(core_discovery_credential_commit_confirmation)
            .transpose()?;
        self.core
            .commit_provider_discovery(
                &parse_session_id(session_id)?,
                credential_confirmation.as_ref(),
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn get_provider_discovery_credential_install_context(
        &self,
        session_id: &str,
    ) -> ShellResult<ProviderDiscoveryCredentialInstallContextDto> {
        self.core
            .get_provider_discovery_credential_install_context(&parse_session_id(session_id)?)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn get_provider_discovery_credential_lease_context(
        &self,
        session_id: &str,
    ) -> ShellResult<ProviderDiscoveryCredentialLeaseContextDto> {
        self.core
            .get_provider_discovery_credential_lease_context(&parse_session_id(session_id)?)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn get_provider_discovery_credential_install_recovery_context(
        &self,
        session_id: &str,
    ) -> ShellResult<ProviderDiscoveryCredentialInstallContextDto> {
        self.core
            .get_provider_discovery_credential_install_recovery_context(&parse_session_id(
                session_id,
            )?)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn get_provider_discovery_credential_compensation_authority(
        &self,
        session_id: &str,
    ) -> ShellResult<ProviderDiscoveryCredentialAuthorityDto> {
        self.core
            .get_provider_discovery_credential_compensation_authority(&parse_session_id(
                session_id,
            )?)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn reserve_provider_discovery_credential_install(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
    ) -> ShellResult<ProviderDiscoveryCredentialInstallContextDto> {
        validate_sha256("commit_plan_sha256", commit_plan_sha256)?;
        let operation_id = DiscoveryOperationId::parse(operation_id)
            .map_err(|_| shell_invalid("operation_id is invalid"))?;
        let attempt_id = DiscoveryCommitAttemptId::parse(commit_attempt_id)
            .map_err(|_| shell_invalid("commit_attempt_id is invalid"))?;
        self.core
            .reserve_provider_discovery_credential_install(
                &parse_session_id(session_id)?,
                expected_revision,
                &operation_id,
                &attempt_id,
                commit_plan_sha256,
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn start_provider_discovery_credential_install(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_reservation_id: &str,
    ) -> ShellResult<ProviderDiscoveryCredentialInstallContextDto> {
        validate_sha256("commit_plan_sha256", commit_plan_sha256)?;
        validate_identifier(
            "native_execution_reservation_id",
            native_execution_reservation_id,
        )?;
        let operation_id = DiscoveryOperationId::parse(operation_id)
            .map_err(|_| shell_invalid("operation_id is invalid"))?;
        let attempt_id = DiscoveryCommitAttemptId::parse(commit_attempt_id)
            .map_err(|_| shell_invalid("commit_attempt_id is invalid"))?;
        self.core
            .start_provider_discovery_credential_install(
                &parse_session_id(session_id)?,
                expected_revision,
                &operation_id,
                &attempt_id,
                commit_plan_sha256,
                native_execution_reservation_id,
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn attest_provider_discovery_credential_install_no_effect(
        &self,
        session_id: &str,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_id: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        validate_sha256("commit_plan_sha256", commit_plan_sha256)?;
        validate_identifier("native_execution_id", native_execution_id)?;
        let operation_id = DiscoveryOperationId::parse(operation_id)
            .map_err(|_| shell_invalid("operation_id is invalid"))?;
        let attempt_id = DiscoveryCommitAttemptId::parse(commit_attempt_id)
            .map_err(|_| shell_invalid("commit_attempt_id is invalid"))?;
        let snapshot = self
            .core
            .attest_provider_discovery_credential_install_no_effect(
                &parse_session_id(session_id)?,
                &operation_id,
                &attempt_id,
                commit_plan_sha256,
                native_execution_id,
            )
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    /// Records an explicit native durability failure for one exact discovery
    /// credential execution. This backend-only API contains no credential
    /// material and cannot be invoked by the webview.
    #[allow(clippy::too_many_arguments)]
    pub fn mark_provider_discovery_credential_install_durability_unknown(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_id: &str,
        connection_id: &str,
        connection_binding_sha256: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        validate_sha256("commit_plan_sha256", commit_plan_sha256)?;
        validate_sha256("connection_binding_sha256", connection_binding_sha256)?;
        validate_identifier("native_execution_id", native_execution_id)?;
        validate_identifier("connection_id", connection_id)?;
        let operation_id = DiscoveryOperationId::parse(operation_id)
            .map_err(|_| shell_invalid("operation_id is invalid"))?;
        let attempt_id = DiscoveryCommitAttemptId::parse(commit_attempt_id)
            .map_err(|_| shell_invalid("commit_attempt_id is invalid"))?;
        let snapshot = self
            .core
            .mark_provider_discovery_credential_install_durability_unknown(
                &parse_session_id(session_id)?,
                expected_revision,
                &operation_id,
                &attempt_id,
                commit_plan_sha256,
                native_execution_id,
                &ProviderConnectionId::from(connection_id),
                connection_binding_sha256,
            )
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn poll_provider_discovery_events(
        &self,
        limit: u32,
    ) -> ShellResult<Vec<DiscoveryOutboxEventDto>> {
        self.core
            .poll_provider_discovery_events(limit, Utc::now())
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn poll_provider_discovery_events_for_session(
        &self,
        session_id: &str,
        limit: u32,
    ) -> ShellResult<Vec<DiscoveryOutboxEventDto>> {
        self.core
            .poll_provider_discovery_events_for_session(
                &parse_session_id(session_id)?,
                limit,
                Utc::now(),
            )
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn ack_provider_discovery_event(&self, event_id: &str) -> ShellResult<bool> {
        let event_id = DiscoveryEventId::parse(event_id)
            .map_err(|_| shell_invalid("discovery event id is invalid"))?;
        self.core
            .ack_provider_discovery_event(&event_id, Utc::now())
            .map_err(ShellError::from)
    }

    pub fn recover_provider_discovery(&self) -> ShellResult<Vec<DiscoveryRecoveryResultDto>> {
        self.core
            .recover_provider_discovery(Utc::now())
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn list_provider_discovery_compensation_steps(
        &self,
        commit_attempt_id: &str,
    ) -> ShellResult<Vec<DiscoveryCompensationRecordDto>> {
        let attempt_id = DiscoveryCommitAttemptId::parse(commit_attempt_id)
            .map_err(|_| shell_invalid("commit_attempt_id is invalid"))?;
        self.core
            .list_provider_discovery_compensation_steps(&attempt_id)
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn continue_provider_discovery_compensation(
        &self,
        session_id: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .continue_provider_discovery_compensation(&parse_session_id(session_id)?)
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn resume_provider_discovery_compensation(
        &self,
        session_id: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        let snapshot = self
            .core
            .resume_provider_discovery_compensation(&parse_session_id(session_id)?)
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn start_provider_discovery_credential_compensation(
        &self,
        session_id: &str,
        step_id: &str,
    ) -> ShellResult<DiscoveryCompensationRecordDto> {
        validate_identifier("compensation_step_id", step_id)?;
        self.core
            .start_provider_discovery_credential_compensation(
                &parse_session_id(session_id)?,
                step_id,
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn complete_provider_discovery_credential_compensation(
        &self,
        session_id: &str,
        step_id: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        validate_identifier("compensation_step_id", step_id)?;
        let snapshot = self
            .core
            .complete_provider_discovery_credential_compensation(
                &parse_session_id(session_id)?,
                step_id,
            )
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn fail_provider_discovery_credential_compensation(
        &self,
        session_id: &str,
        step_id: &str,
        failure: DiscoveryFailureDto,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        validate_identifier("compensation_step_id", step_id)?;
        let snapshot = self
            .core
            .fail_provider_discovery_credential_compensation(
                &parse_session_id(session_id)?,
                step_id,
                failure.into(),
            )
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }

    pub fn mark_provider_discovery_credential_compensation_unknown(
        &self,
        session_id: &str,
        step_id: &str,
    ) -> ShellResult<ProviderDiscoverySessionDto> {
        validate_identifier("compensation_step_id", step_id)?;
        let snapshot = self
            .core
            .mark_provider_discovery_credential_compensation_unknown(
                &parse_session_id(session_id)?,
                step_id,
            )
            .map_err(ShellError::from)?;
        self.project_provider_discovery_session(snapshot)
    }
}

fn core_credential_access_authority(
    context: ProviderCredentialAccessAuthorityContext,
) -> lorepia_core::ProviderCredentialAccessAuthority {
    lorepia_core::ProviderCredentialAccessAuthority {
        authority_id: context.authority_id,
        connection_binding_sha256: context.connection_binding_sha256,
    }
}

fn core_discovery_credential_commit_confirmation(
    context: &ProviderDiscoveryCredentialCommitConfirmationDto,
) -> ShellResult<ProviderDiscoveryCredentialCommitConfirmation> {
    validate_sha256("commit_plan_sha256", &context.commit_plan_sha256)?;
    validate_sha256(
        "connection_binding_sha256",
        &context.connection_binding_sha256,
    )?;
    validate_identifier("connection_id", &context.connection_id)?;
    validate_identifier("native_execution_id", &context.native_execution_id)?;
    Ok(ProviderDiscoveryCredentialCommitConfirmation {
        operation_id: DiscoveryOperationId::parse(&context.operation_id)
            .map_err(|_| shell_invalid("operation_id is invalid"))?,
        native_execution_id: context.native_execution_id.clone(),
        commit_attempt_id: DiscoveryCommitAttemptId::parse(&context.commit_attempt_id)
            .map_err(|_| shell_invalid("commit_attempt_id is invalid"))?,
        commit_plan_sha256: context.commit_plan_sha256.clone(),
        connection_id: ProviderConnectionId::from(context.connection_id.clone()),
        connection_binding_sha256: context.connection_binding_sha256.clone(),
    })
}

#[allow(clippy::too_many_arguments)]
fn sanitized_discovery_input(
    connection_id: String,
    display_name: String,
    site_url: String,
    docs_url: Option<String>,
    credential_binding_requested: bool,
    preferred_assistant: Option<String>,
    connection_options: ProviderDiscoveryConnectionOptionsInput,
    supplied_evidence_ids: Vec<String>,
) -> ShellResult<SanitizedDiscoveryInput> {
    validate_identifier("connection_id", &connection_id)?;
    let connection_id = ProviderConnectionId::from(connection_id);
    Ok(SanitizedDiscoveryInput {
        credential_ref: credential_binding_requested
            .then(|| CredentialRef(connection_id.as_str().to_owned())),
        connection_id,
        display_name,
        site_url: parse_http_url("site_url", &site_url)?,
        docs_url: docs_url
            .map(|url| parse_http_url("docs_url", &url))
            .transpose()?,
        preferred_assistant: preferred_assistant
            .map(|id| {
                validate_identifier("preferred_assistant", &id)?;
                Ok::<_, ShellError>(ModelRouteId::from(id))
            })
            .transpose()?,
        connection_options: connection_options.try_into()?,
        supplied_evidence_ids: parse_evidence_ids(supplied_evidence_ids)?,
    })
}

fn parse_http_url(field: &str, value: &str) -> ShellResult<HttpUrl> {
    HttpUrl::parse(value).map_err(|_| shell_invalid(&format!("{field} is invalid")))
}

fn parse_local_network_approval(
    value: ProviderLocalNetworkApprovalInput,
) -> ShellResult<ProviderLocalNetworkApproval> {
    Ok(ProviderLocalNetworkApproval {
        origin: CanonicalOrigin::parse(&value.origin)
            .map_err(|_| shell_invalid("local network approval origin is invalid"))?,
        addresses: value
            .addresses
            .into_iter()
            .map(|address| {
                IpAddr::from_str(&address)
                    .map_err(|_| shell_invalid("local network approval address is invalid"))
            })
            .collect::<ShellResult<Vec<_>>>()?,
    })
}

fn parse_evidence_ids(values: Vec<String>) -> ShellResult<Vec<EvidenceId>> {
    values
        .into_iter()
        .map(|value| {
            validate_identifier("evidence_id", &value)?;
            Ok(EvidenceId::from(value))
        })
        .collect()
}

fn parse_session_id(value: &str) -> ShellResult<DiscoverySessionId> {
    validate_identifier("discovery_session_id", value)?;
    Ok(DiscoverySessionId::from(value))
}

fn parse_approval_id(value: &str) -> ShellResult<DiscoveryApprovalId> {
    DiscoveryApprovalId::parse(value).map_err(|_| shell_invalid("discovery approval id is invalid"))
}

fn validate_sha256(field: &str, value: &str) -> ShellResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(shell_invalid(&format!(
            "{field} is not a lowercase SHA-256 digest"
        )));
    }
    Ok(())
}

fn shell_invalid(message: &str) -> ShellError {
    lorepia_core::CoreError::invalid(message).into()
}

fn action_required_for_snapshot(
    snapshot: &DiscoverySessionSnapshot,
) -> Option<DiscoveryActionRequiredDto> {
    match snapshot.session.state {
        DiscoveryState::AwaitingTemplateSelection => {
            Some(DiscoveryActionRequiredDto::simple("select_template"))
        }
        DiscoveryState::AwaitingMoreEvidence => {
            Some(DiscoveryActionRequiredDto::simple("supply_more_evidence"))
        }
        DiscoveryState::AwaitingAssistantConsent => {
            Some(DiscoveryActionRequiredDto::simple("approve_assistant"))
        }
        DiscoveryState::AwaitingCredentialOriginApproval => Some(
            DiscoveryActionRequiredDto::simple("approve_credential_origin"),
        ),
        DiscoveryState::AwaitingProbeConsent => {
            Some(DiscoveryActionRequiredDto::simple("approve_probes"))
        }
        DiscoveryState::AwaitingReview => Some(DiscoveryActionRequiredDto::simple("review")),
        DiscoveryState::Interrupted => {
            snapshot
                .session
                .recovery
                .as_ref()
                .map(|checkpoint| DiscoveryActionRequiredDto {
                    kind: "restart_interrupted".to_owned(),
                    operation: Some(discovery_operation_name(checkpoint.operation).to_owned()),
                })
        }
        DiscoveryState::UnknownOutcome => {
            snapshot
                .session
                .unknown_operation
                .map(|operation| DiscoveryActionRequiredDto {
                    kind: "reconcile_unknown_outcome".to_owned(),
                    operation: Some(discovery_operation_name(operation).to_owned()),
                })
        }
        _ => None,
    }
}

fn discovery_phase_rank(state: DiscoveryState) -> usize {
    match state {
        DiscoveryState::Draft
        | DiscoveryState::ResolvingKnownProvider
        | DiscoveryState::AwaitingTemplateSelection => 0,
        DiscoveryState::FetchingDocuments
        | DiscoveryState::ExtractingEvidence
        | DiscoveryState::AwaitingMoreEvidence => 1,
        DiscoveryState::AwaitingAssistantConsent
        | DiscoveryState::BuildingDeterministicManifestDraft
        | DiscoveryState::BuildingAssistantManifestDraft
        | DiscoveryState::ValidatingManifest
        | DiscoveryState::AwaitingCredentialOriginApproval => 2,
        DiscoveryState::ListingModels
        | DiscoveryState::AwaitingProbeConsent
        | DiscoveryState::ProbingCapabilities => 3,
        DiscoveryState::AwaitingReview => 4,
        DiscoveryState::Committing
        | DiscoveryState::Compensating
        | DiscoveryState::Ready
        | DiscoveryState::Failed
        | DiscoveryState::Cancelled
        | DiscoveryState::Interrupted
        | DiscoveryState::UnknownOutcome => 5,
    }
}

fn discovery_steps(state: DiscoveryState) -> Vec<DiscoveryStepDto> {
    let current = discovery_phase_rank(state);
    [
        ("provider", "provider.discovery.step.provider"),
        ("evidence", "provider.discovery.step.evidence"),
        ("manifest", "provider.discovery.step.manifest"),
        ("models", "provider.discovery.step.models"),
        ("review", "provider.discovery.step.review"),
        ("commit", "provider.discovery.step.commit"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (id, title_key))| DiscoveryStepDto {
        id: id.to_owned(),
        title_key: title_key.to_owned(),
        state: if state == DiscoveryState::Ready || index < current {
            "completed"
        } else if index == current {
            "current"
        } else {
            "pending"
        }
        .to_owned(),
    })
    .collect()
}

const fn discovery_network_mode_name(value: lorepia_core::ProviderNetworkMode) -> &'static str {
    match value {
        lorepia_core::ProviderNetworkMode::Public => "public",
        lorepia_core::ProviderNetworkMode::LocalLoopback => "local_loopback",
        lorepia_core::ProviderNetworkMode::ApprovedLocalNetwork => "approved_local_network",
    }
}

const fn assistant_checkpoint_name(value: DiscoveryAssistantCheckpoint) -> &'static str {
    match value {
        DiscoveryAssistantCheckpoint::Ready => "ready",
        DiscoveryAssistantCheckpoint::AwaitingAssistant => "awaiting_assistant",
        DiscoveryAssistantCheckpoint::AwaitingToolResult => "awaiting_tool_result",
        DiscoveryAssistantCheckpoint::AwaitingMoreEvidence => "awaiting_more_evidence",
        DiscoveryAssistantCheckpoint::AwaitingRetryConsent => "awaiting_retry_consent",
        DiscoveryAssistantCheckpoint::DraftReady => "draft_ready",
    }
}

const fn assistant_resume_action_name(
    value: ProviderDiscoveryAssistantResumeAction,
) -> &'static str {
    match value {
        ProviderDiscoveryAssistantResumeAction::ApproveConsent => "approve_consent",
        ProviderDiscoveryAssistantResumeAction::RunAssistant => "run_assistant",
        ProviderDiscoveryAssistantResumeAction::WaitForAssistantOutcome => {
            "wait_for_assistant_outcome"
        }
        ProviderDiscoveryAssistantResumeAction::ResumeCoreHostAction => "resume_core_host_action",
        ProviderDiscoveryAssistantResumeAction::SupplyMoreEvidence => "supply_more_evidence",
        ProviderDiscoveryAssistantResumeAction::ApproveRetry => "approve_retry",
        ProviderDiscoveryAssistantResumeAction::ReviewDraft => "review_draft",
        ProviderDiscoveryAssistantResumeAction::RestartInterrupted => "restart_interrupted",
        ProviderDiscoveryAssistantResumeAction::ResolveUnknownOutcome => "resolve_unknown_outcome",
    }
}

const fn assistant_confidence_level_name(value: ConfidenceLevel) -> &'static str {
    match value {
        ConfidenceLevel::Unknown => "unknown",
        ConfidenceLevel::Low => "low",
        ConfidenceLevel::Medium => "medium",
        ConfidenceLevel::High => "high",
    }
}

const fn assistant_api_family_name(value: ApiFamily) -> &'static str {
    match value {
        ApiFamily::OpenAiResponses => "openai_responses",
        ApiFamily::OpenAiChatCompletions => "openai_chat_completions",
        ApiFamily::AnthropicMessages => "anthropic_messages",
        ApiFamily::GeminiGenerateContent => "gemini_generate_content",
        ApiFamily::OllamaNative => "ollama_native",
    }
}

const fn manifest_source_kind_name(value: ManifestSourceKind) -> &'static str {
    match value {
        ManifestSourceKind::OfficialSite => "official_site",
        ManifestSourceKind::OfficialDocumentation => "official_documentation",
        ManifestSourceKind::SignedCatalog => "signed_catalog",
        ManifestSourceKind::UserSupplied => "user_supplied",
    }
}

const fn assistant_http_method_name(value: HttpMethod) -> &'static str {
    match value {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
    }
}

const fn assistant_decoder_name(value: DecoderId) -> &'static str {
    match value {
        DecoderId::OpenAiJsonV1 => "openai_json_v1",
        DecoderId::OpenAiSseV1 => "openai_sse_v1",
        DecoderId::AnthropicJsonV1 => "anthropic_json_v1",
        DecoderId::AnthropicSseV1 => "anthropic_sse_v1",
        DecoderId::GeminiJsonV1 => "gemini_json_v1",
        DecoderId::GeminiSseV1 => "gemini_sse_v1",
        DecoderId::OllamaJsonV1 => "ollama_json_v1",
        DecoderId::OllamaJsonlV1 => "ollama_jsonl_v1",
    }
}

const fn assistant_review_check_name(value: DraftReviewCheck) -> &'static str {
    match value {
        DraftReviewCheck::ManifestValidation => "manifest_validation",
        DraftReviewCheck::UrlPolicyValidation => "url_policy_validation",
        DraftReviewCheck::CredentialOriginApproval => "credential_origin_approval",
        DraftReviewCheck::UserReview => "user_review",
    }
}

const fn assistant_persistence_name(value: DraftPersistence) -> &'static str {
    match value {
        DraftPersistence::BlockedUntilChecksPass => "blocked_until_checks_pass",
    }
}

const fn discovery_state_name(value: DiscoveryState) -> &'static str {
    match value {
        DiscoveryState::Draft => "draft",
        DiscoveryState::ResolvingKnownProvider => "resolving_known_provider",
        DiscoveryState::AwaitingTemplateSelection => "awaiting_template_selection",
        DiscoveryState::FetchingDocuments => "fetching_documents",
        DiscoveryState::ExtractingEvidence => "extracting_evidence",
        DiscoveryState::AwaitingMoreEvidence => "awaiting_more_evidence",
        DiscoveryState::AwaitingAssistantConsent => "awaiting_assistant_consent",
        DiscoveryState::BuildingDeterministicManifestDraft => {
            "building_deterministic_manifest_draft"
        }
        DiscoveryState::BuildingAssistantManifestDraft => "building_assistant_manifest_draft",
        DiscoveryState::ValidatingManifest => "validating_manifest",
        DiscoveryState::AwaitingCredentialOriginApproval => "awaiting_credential_origin_approval",
        DiscoveryState::ListingModels => "listing_models",
        DiscoveryState::AwaitingProbeConsent => "awaiting_probe_consent",
        DiscoveryState::ProbingCapabilities => "probing_capabilities",
        DiscoveryState::AwaitingReview => "awaiting_review",
        DiscoveryState::Committing => "committing",
        DiscoveryState::Compensating => "compensating",
        DiscoveryState::Ready => "ready",
        DiscoveryState::Failed => "failed",
        DiscoveryState::Cancelled => "cancelled",
        DiscoveryState::Interrupted => "interrupted",
        DiscoveryState::UnknownOutcome => "unknown_outcome",
    }
}

const fn discovery_operation_name(value: DiscoveryOperationKind) -> &'static str {
    match value {
        DiscoveryOperationKind::ResolveKnownProvider => "resolve_known_provider",
        DiscoveryOperationKind::FetchDocuments => "fetch_documents",
        DiscoveryOperationKind::ExtractEvidence => "extract_evidence",
        DiscoveryOperationKind::BuildDeterministicManifestDraft => {
            "build_deterministic_manifest_draft"
        }
        DiscoveryOperationKind::BuildAssistantManifestDraft => "build_assistant_manifest_draft",
        DiscoveryOperationKind::ValidateManifest => "validate_manifest",
        DiscoveryOperationKind::ListModels => "list_models",
        DiscoveryOperationKind::ProbeCapabilities => "probe_capabilities",
        DiscoveryOperationKind::AtomicCommit => "atomic_commit",
        DiscoveryOperationKind::Compensation => "compensation",
    }
}

const fn discovery_operation_status_name(value: DiscoveryOperationStatus) -> &'static str {
    match value {
        DiscoveryOperationStatus::Prepared => "prepared",
        DiscoveryOperationStatus::Started => "started",
        DiscoveryOperationStatus::Succeeded => "succeeded",
        DiscoveryOperationStatus::Failed => "failed",
        DiscoveryOperationStatus::Interrupted => "interrupted",
        DiscoveryOperationStatus::OutcomeUnknown => "outcome_unknown",
    }
}

const fn discovery_commit_phase_name(value: DiscoveryCommitPhase) -> &'static str {
    match value {
        DiscoveryCommitPhase::Prepared => "prepared",
        DiscoveryCommitPhase::DatabaseApplied => "database_applied",
        DiscoveryCommitPhase::CredentialReferenceApplied => "credential_reference_applied",
        DiscoveryCommitPhase::Completed => "completed",
        DiscoveryCommitPhase::CompensationRequired => "compensation_required",
        DiscoveryCommitPhase::Compensating => "compensating",
        DiscoveryCommitPhase::Compensated => "compensated",
        DiscoveryCommitPhase::OutcomeUnknown => "outcome_unknown",
    }
}

const fn discovery_evidence_kind_name(value: DiscoveryEvidenceKind) -> &'static str {
    match value {
        DiscoveryEvidenceKind::HtmlDocument => "html_document",
        DiscoveryEvidenceKind::JsonDocument => "json_document",
        DiscoveryEvidenceKind::YamlDocument => "yaml_document",
        DiscoveryEvidenceKind::XmlDocument => "xml_document",
        DiscoveryEvidenceKind::PlainTextDocument => "plain_text_document",
        DiscoveryEvidenceKind::JsonSchema => "json_schema",
        DiscoveryEvidenceKind::OpenApi => "open_api",
    }
}

const fn discovery_warning_name(value: DiscoveryWarning) -> &'static str {
    match value {
        DiscoveryWarning::AssistantDeclined => "assistant_declined",
        DiscoveryWarning::ProbesSkipped => "probes_skipped",
        DiscoveryWarning::CompensationRequired => "compensation_required",
        DiscoveryWarning::ExplicitRestartRequired => "explicit_restart_required",
        DiscoveryWarning::UnknownExternalOutcome => "unknown_external_outcome",
    }
}

const fn discovery_review_change_kind_name(value: DiscoveryReviewChangeKind) -> &'static str {
    match value {
        DiscoveryReviewChangeKind::Add => "add",
        DiscoveryReviewChangeKind::Update => "update",
        DiscoveryReviewChangeKind::Deprecate => "deprecate",
        DiscoveryReviewChangeKind::PreserveMissing => "preserve_missing",
    }
}

const fn discovery_compensation_kind_name(value: DiscoveryCompensationKind) -> &'static str {
    match value {
        DiscoveryCompensationKind::RemoveCredentialSlot => "remove_credential_slot",
        DiscoveryCompensationKind::RemoveConnectionGraph => "remove_connection_graph",
        DiscoveryCompensationKind::RestorePreviousSelection => "restore_previous_selection",
    }
}

const fn discovery_compensation_status_name(value: DiscoveryCompensationStatus) -> &'static str {
    match value {
        DiscoveryCompensationStatus::Pending => "pending",
        DiscoveryCompensationStatus::InProgress => "in_progress",
        DiscoveryCompensationStatus::Completed => "completed",
        DiscoveryCompensationStatus::Failed => "failed",
        DiscoveryCompensationStatus::OutcomeUnknown => "outcome_unknown",
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use lorepia_core::{
        AssistantHostAction, AssistantToolCall, CoreConfig, DiscoveryAssistantCheckpoint,
        DiscoveryOperationId, DiscoveryOperationKind, DiscoveryRecoveryCheckpoint,
        DiscoverySessionId, DiscoveryState, EvidenceId, HttpUrl, ProviderConnectionId,
        ProviderDiscoveryAssistantResumeAction, ProviderDiscoveryAssistantResumeBoundary,
        ProviderDiscoveryConnectionOptions, ProviderDiscoverySession, SanitizedDiscoveryInput,
        UnresolvedQuestion,
    };

    use crate::{ProviderNetworkModeInput, ShellApi, ShellErrorCode};

    use super::{
        BeginProviderDiscoveryInput, BeginProviderDiscoverySourceInput,
        ContinueProviderDiscoveryActionInput, ContinueProviderDiscoveryInput,
        DiscoveryAssistantFailureKindInput, DiscoveryAssistantHostActionDto,
        DiscoveryAssistantInterruptionOutcomeInput, ProviderDiscoveryConnectionOptionsInput,
        ProviderDiscoveryCredentialCommitConfirmationDto,
        ProviderDiscoveryCredentialInstallContextDto, ProviderDiscoverySessionDto,
        core_discovery_credential_commit_confirmation,
    };

    #[test]
    fn backend_commit_confirmation_keeps_operation_and_attempt_identity_separate() {
        let context = ProviderDiscoveryCredentialInstallContextDto {
            session_id: "session".to_owned(),
            session_revision: 7,
            operation_id: "00000000-0000-4000-8000-000000000071".to_owned(),
            operation_status: "started".to_owned(),
            native_execution_reservation_id: Some("native-execution-71".to_owned()),
            native_execution_id: Some("native-execution-71".to_owned()),
            commit_attempt_id: "00000000-0000-4000-8000-000000000072".to_owned(),
            commit_plan_sha256: "a".repeat(64),
            commit_phase: "prepared".to_owned(),
            connection_id: "connection".to_owned(),
            connection_binding_sha256: "b".repeat(64),
        };
        let confirmation = ProviderDiscoveryCredentialCommitConfirmationDto::try_from(&context)
            .expect("started context has native execution authority");
        let core = core_discovery_credential_commit_confirmation(&confirmation)
            .expect("parse exact backend confirmation");

        assert_eq!(core.operation_id.to_string(), context.operation_id);
        assert_eq!(
            Some(core.native_execution_id.as_str()),
            context.native_execution_id.as_deref()
        );
        assert_eq!(
            core.commit_attempt_id.to_string(),
            context.commit_attempt_id
        );
        assert_ne!(
            core.operation_id.to_string(),
            core.commit_attempt_id.to_string()
        );
        assert_eq!(core.commit_plan_sha256, context.commit_plan_sha256);
        assert_eq!(core.connection_id.as_str(), context.connection_id);
        assert_eq!(
            core.connection_binding_sha256,
            context.connection_binding_sha256
        );

        let mut prepared = context.clone();
        prepared.operation_status = "prepared".to_owned();
        prepared.native_execution_id = None;
        ProviderDiscoveryCredentialCommitConfirmationDto::try_from(&prepared)
            .expect_err("a reservation is not Started confirmation authority");

        let mut forged = context;
        forged.native_execution_id = Some("native-execution-forged".to_owned());
        ProviderDiscoveryCredentialCommitConfirmationDto::try_from(&forged)
            .expect_err("confirmation must reject an execution other than reserved B");
    }

    #[test]
    fn backend_credential_lease_context_projects_exact_precommit_authority() {
        let root = tempfile::tempdir().expect("temporary root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("shell");
        let selecting = shell
            .begin_provider_discovery(BeginProviderDiscoveryInput {
                connection_id: "shell-precommit-lease".to_owned(),
                display_name: "Shell precommit lease".to_owned(),
                site_url: "https://openrouter.ai/".to_owned(),
                docs_url: None,
                credential_binding_requested: true,
                preferred_assistant: None,
                connection_options: ProviderDiscoveryConnectionOptionsInput {
                    values: Vec::new(),
                    api_base_path: None,
                    timeout_seconds: 30,
                    network_mode: ProviderNetworkModeInput::Public,
                    local_network_approval: None,
                },
                supplied_evidence_ids: Vec::new(),
                source: BeginProviderDiscoverySourceInput::KnownProvider {
                    template_id: "openrouter-v1".to_owned(),
                },
            })
            .expect("begin known-provider discovery");
        let candidate = shell
            .list_provider_discovery_candidates(&selecting.id)
            .expect("list template candidates")
            .into_iter()
            .find(|candidate| {
                matches!(
                    &candidate.summary,
                    super::DiscoveryCandidateSummaryDto::ProviderTemplate { template_id, .. }
                        if template_id == "openrouter-v1"
                )
            })
            .expect("OpenRouter candidate");
        let approval = shell
            .continue_provider_discovery(
                ContinueProviderDiscoveryInput {
                    session_id: selecting.id,
                    action_id: "00000000-0000-4000-8000-000000000001".to_owned(),
                    expected_revision: selecting.revision,
                    action: ContinueProviderDiscoveryActionInput::SelectTemplate {
                        candidate_id: candidate.id,
                    },
                },
                None,
            )
            .expect("select OpenRouter template");
        assert_eq!(approval.state, "awaiting_credential_origin_approval");

        let context = shell
            .get_provider_discovery_credential_lease_context(&approval.id)
            .expect("backend credential lease context");
        assert_eq!(context.session_id, approval.id);
        assert_eq!(context.connection_id, "shell-precommit-lease");
        assert_eq!(context.credential_api_origin, "https://openrouter.ai");
        assert!(!context.credential_origin_approval_id.is_empty());
        assert_eq!(context.credential_origin_grant_sha256.len(), 64);
        assert_eq!(context.connection_binding_sha256.len(), 64);

        assert_eq!(
            shell
                .get_provider_discovery_credential_lease_context("wrong-session")
                .expect_err("wrong session must not reuse another discovery lease")
                .code,
            ShellErrorCode::NotFound
        );
    }

    #[test]
    fn session_scoped_outbox_poll_validates_its_session_and_limit() {
        let root = tempfile::tempdir().expect("temporary root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("shell");

        for invalid_session_id in [String::new(), " padded".to_owned(), "s".repeat(129)] {
            assert_eq!(
                shell
                    .poll_provider_discovery_events_for_session(&invalid_session_id, 100)
                    .expect_err("invalid session id must be rejected")
                    .code,
                ShellErrorCode::InvalidInput
            );
        }
        for invalid_limit in [0, 1_001] {
            assert_eq!(
                shell
                    .poll_provider_discovery_events_for_session("missing-session", invalid_limit)
                    .expect_err("invalid limit must be rejected")
                    .code,
                ShellErrorCode::InvalidInput
            );
        }
        assert!(
            shell
                .poll_provider_discovery_events_for_session("missing-session", 100)
                .expect("valid absent session has no events")
                .is_empty()
        );
    }

    #[test]
    fn session_projection_omits_credential_reference_and_private_draft() {
        let canary = "credential-reference-discovery-canary";
        let input = SanitizedDiscoveryInput {
            connection_id: ProviderConnectionId::from(canary),
            display_name: "Provider".to_owned(),
            site_url: HttpUrl::parse("https://example.test/").expect("site"),
            docs_url: None,
            credential_ref: Some(lorepia_core::CredentialRef(canary.to_owned())),
            preferred_assistant: None,
            connection_options: ProviderDiscoveryConnectionOptions::default(),
            supplied_evidence_ids: Vec::new(),
        };
        let mut session = ProviderDiscoverySession::new(DiscoverySessionId::from("session"), input)
            .expect("session");
        session.state = DiscoveryState::Draft;
        let now = Utc::now();
        let dto = ProviderDiscoverySessionDto::from_snapshot(
            lorepia_core::DiscoverySessionSnapshot {
                session,
                active_operation_id: None,
                draft_json: Some(serde_json::json!({
                    "assistant_private_payload": "assistant-private-canary"
                })),
                review: None,
                created_at: now,
                updated_at: now,
            },
            None,
        );
        let json = serde_json::to_string(&dto).expect("serialize");

        assert!(dto.credential_binding_requested);
        assert!(dto.has_private_draft);
        assert!(!json.contains("credential_ref"));
        assert!(!json.contains("assistant-private-canary"));
        // The logical connection ID remains visible; only the credential
        // reference field/value channel is removed.
        assert!(json.contains(canary));
    }

    #[test]
    fn session_projection_preserves_restart_critical_state_without_private_payloads() {
        let input = SanitizedDiscoveryInput {
            connection_id: ProviderConnectionId::from("connection"),
            display_name: "Provider".to_owned(),
            site_url: HttpUrl::parse("https://example.test/").expect("site"),
            docs_url: Some(HttpUrl::parse("https://example.test/docs").expect("documentation URL")),
            credential_ref: None,
            preferred_assistant: Some("assistant-route".into()),
            connection_options: ProviderDiscoveryConnectionOptions {
                timeout_seconds: 41,
                api_base_path: Some(
                    lorepia_core::EndpointPath::parse("/v1").expect("API base path"),
                ),
                ..ProviderDiscoveryConnectionOptions::default()
            },
            supplied_evidence_ids: vec![EvidenceId::from("evidence-1")],
        };
        let mut session =
            ProviderDiscoverySession::new(DiscoverySessionId::from("restart-session"), input)
                .expect("session");
        session.state = DiscoveryState::Interrupted;
        session.recovery = Some(DiscoveryRecoveryCheckpoint {
            interrupted_state: DiscoveryState::BuildingAssistantManifestDraft,
            operation: DiscoveryOperationKind::BuildAssistantManifestDraft,
        });
        let now = Utc::now();
        let dto = ProviderDiscoverySessionDto::from_snapshot(
            lorepia_core::DiscoverySessionSnapshot {
                session,
                active_operation_id: Some(
                    DiscoveryOperationId::parse("active-operation").expect("operation ID"),
                ),
                draft_json: Some(serde_json::json!({
                    "raw_evidence": "raw-evidence-canary",
                    "assistant_prompt": "assistant-prompt-canary"
                })),
                review: None,
                created_at: now,
                updated_at: now,
            },
            Some(ProviderDiscoveryAssistantResumeBoundary {
                checkpoint: Some(DiscoveryAssistantCheckpoint::Ready),
                action: ProviderDiscoveryAssistantResumeAction::RestartInterrupted,
                questions: vec![UnresolvedQuestion {
                    id: "question-1".to_owned(),
                    field: None,
                    question: "Provide the official API documentation.".to_owned(),
                    required_evidence: "Official documentation URL".to_owned(),
                }],
                draft_review: None,
            }),
        );
        let json = serde_json::to_string(&dto).expect("serialize");

        assert_eq!(dto.snapshot_schema_version, 3);
        assert_eq!(dto.connection_options.timeout_seconds, 41);
        assert_eq!(dto.connection_options.api_base_path.as_deref(), Some("/v1"));
        assert_eq!(dto.supplied_evidence_ids, ["evidence-1"]);
        assert_eq!(dto.active_operation_id.as_deref(), Some("active-operation"));
        assert_eq!(
            dto.action_required
                .as_ref()
                .map(|action| action.kind.as_str()),
            Some("restart_interrupted")
        );
        assert_eq!(
            dto.assistant_resume_boundary
                .as_ref()
                .map(|boundary| boundary.action.as_str()),
            Some("restart_interrupted")
        );
        assert_eq!(dto.steps.len(), 6);
        assert!(!json.contains("raw-evidence-canary"));
        assert!(!json.contains("assistant-prompt-canary"));
        assert!(!json.contains("draft_json"));
    }

    #[test]
    fn assistant_surface_is_core_backed_and_never_serializes_tool_calls() {
        let escaped = DiscoveryAssistantHostActionDto::try_from(AssistantHostAction::ExecuteTool {
            session_id: DiscoverySessionId::from("session"),
            call_id: 7,
            call: AssistantToolCall::ShowUnresolvedQuestions,
        })
        .expect_err("Core-owned tool action must not cross the shell boundary");
        assert_eq!(escaped.code, ShellErrorCode::Internal);

        let root = tempfile::tempdir().expect("temporary root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("shell");
        let missing = "missing-assistant-session";
        assert_eq!(
            shell
                .get_provider_discovery_assistant_resume_boundary(missing)
                .expect_err("missing boundary")
                .code,
            ShellErrorCode::NotFound
        );
        assert_eq!(
            shell
                .resume_provider_discovery_assistant_core_host_action(missing)
                .expect_err("missing host action")
                .code,
            ShellErrorCode::NotFound
        );
        assert_eq!(
            shell
                .approve_provider_discovery_assistant_retry(missing)
                .expect_err("missing retry")
                .code,
            ShellErrorCode::NotFound
        );
        assert_eq!(
            shell
                .request_provider_discovery_assistant_revision(missing)
                .expect_err("missing revision")
                .code,
            ShellErrorCode::NotFound
        );
        assert_eq!(
            shell
                .accept_provider_discovery_assistant_draft(missing)
                .expect_err("missing draft")
                .code,
            ShellErrorCode::NotFound
        );
        assert_eq!(
            shell
                .record_provider_discovery_assistant_failure(
                    missing,
                    DiscoveryAssistantFailureKindInput::Transport,
                    true,
                )
                .expect_err("missing failure")
                .code,
            ShellErrorCode::NotFound
        );
        assert_eq!(
            shell
                .interrupt_provider_discovery_assistant(
                    missing,
                    DiscoveryAssistantInterruptionOutcomeInput::ExternalOutcomeUnknown,
                )
                .expect_err("missing interruption")
                .code,
            ShellErrorCode::NotFound
        );
        assert_eq!(
            shell
                .restart_provider_discovery_assistant_after_interruption(missing)
                .expect_err("missing restart")
                .code,
            ShellErrorCode::NotFound
        );
    }
}
