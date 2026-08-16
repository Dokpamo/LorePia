use chrono::{DateTime, Utc};
use lorepia_core::{
    ApiFamily, ConnectionBoundCredential, ModelSyncDiff, ModelSyncEvent, ModelSyncFailure,
    ModelSyncJob, ModelSyncJobId, ModelSyncReview, ModelSyncSourceProvenance, ModelSyncState,
    ProviderConnectionId, ProviderCredentialAccessAuthority,
};
use serde::{Deserialize, Serialize};

use crate::{
    GenerationPresetDto, ModelRouteDto, ProviderConnectionDto,
    ProviderCredentialAccessAuthorityContext, SecretCredential, ShellApi, ShellError, ShellResult,
    TaskCredentialLease, api::validate_identifier,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncStartedDto {
    pub job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncFailureDto {
    pub code: String,
    pub message_key: String,
    pub recoverable: bool,
}

impl From<ModelSyncFailure> for ModelSyncFailureDto {
    fn from(value: ModelSyncFailure) -> Self {
        Self {
            code: value.code,
            message_key: value.message_key,
            recoverable: value.recoverable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncSourceProvenanceDto {
    pub source: String,
    pub api_family: String,
    pub api_origin: String,
    pub endpoint_path: String,
    pub pages_fetched: u32,
    pub response_bytes: u64,
}

impl From<ModelSyncSourceProvenance> for ModelSyncSourceProvenanceDto {
    fn from(value: ModelSyncSourceProvenance) -> Self {
        Self {
            source: value.source,
            api_family: api_family_name(value.api_family).to_owned(),
            api_origin: value.api_origin.to_string(),
            endpoint_path: value.endpoint_path.as_str().to_owned(),
            pages_fetched: value.pages_fetched,
            response_bytes: value.response_bytes,
        }
    }
}

/// Redacted review material for one exact Core review digest.
///
/// Core's credential reference and bounded raw provider metadata are omitted;
/// the digest remains the exact hash which must be echoed for approval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncDiffDto {
    pub connection_id: String,
    pub expected_connection: ProviderConnectionDto,
    pub expected_model_routes: Vec<ModelRouteDto>,
    pub observed_at: DateTime<Utc>,
    pub listed_routes: Vec<ModelRouteDto>,
    pub newly_seen_model_route_ids: Vec<String>,
    pub missing_model_route_ids: Vec<String>,
    pub initial_presets: Vec<GenerationPresetDto>,
    pub capability_observation_count: u32,
    pub routes_requiring_preset_configuration: Vec<String>,
    pub provenance: ModelSyncSourceProvenanceDto,
}

impl From<ModelSyncDiff> for ModelSyncDiffDto {
    fn from(value: ModelSyncDiff) -> Self {
        Self {
            connection_id: value.connection_id.0,
            expected_connection: value.expected_connection.into(),
            expected_model_routes: value
                .expected_model_routes
                .into_iter()
                .map(Into::into)
                .collect(),
            observed_at: value.observed_at,
            listed_routes: value.listed_routes.into_iter().map(Into::into).collect(),
            newly_seen_model_route_ids: value
                .newly_seen_model_route_ids
                .into_iter()
                .map(|id| id.0)
                .collect(),
            missing_model_route_ids: value
                .missing_model_route_ids
                .into_iter()
                .map(|id| id.0)
                .collect(),
            initial_presets: value.initial_presets.into_iter().map(Into::into).collect(),
            capability_observation_count: u32::try_from(value.capability_observations.len())
                .unwrap_or(u32::MAX),
            routes_requiring_preset_configuration: value
                .routes_requiring_preset_configuration
                .into_iter()
                .map(|id| id.0)
                .collect(),
            provenance: value.provenance.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncReviewDto {
    pub sha256: String,
    pub diff: ModelSyncDiffDto,
}

impl From<ModelSyncReview> for ModelSyncReviewDto {
    fn from(value: ModelSyncReview) -> Self {
        Self {
            sha256: value.sha256,
            diff: value.diff.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncJobDto {
    pub id: String,
    pub connection_id: String,
    pub state: String,
    pub revision: u64,
    pub review: Option<ModelSyncReviewDto>,
    pub failure: Option<ModelSyncFailureDto>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ModelSyncJob> for ModelSyncJobDto {
    fn from(value: ModelSyncJob) -> Self {
        Self {
            id: value.id.0,
            connection_id: value.connection_id.0,
            state: model_sync_state_name(value.state).to_owned(),
            revision: value.revision,
            review: value.review.map(Into::into),
            failure: value.failure.map(Into::into),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncProgressDto {
    pub completed_steps: u32,
    pub total_steps: u32,
    pub message_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncEventDto {
    pub version: u32,
    pub job_id: String,
    pub sequence: u64,
    pub job_revision: u64,
    pub redaction_version: u32,
    pub state: String,
    pub progress: ModelSyncProgressDto,
    pub review_sha256: Option<String>,
    pub failure: Option<ModelSyncFailureDto>,
    pub emitted_at: DateTime<Utc>,
}

impl From<ModelSyncEvent> for ModelSyncEventDto {
    fn from(value: ModelSyncEvent) -> Self {
        Self {
            version: value.version,
            job_id: value.job_id.0,
            sequence: value.sequence,
            job_revision: value.job_revision,
            redaction_version: value.redaction_version,
            state: model_sync_state_name(value.state).to_owned(),
            progress: ModelSyncProgressDto {
                completed_steps: value.progress.completed_steps,
                total_steps: value.progress.total_steps,
                message_key: value.progress.message_key,
            },
            review_sha256: value.review_sha256,
            failure: value.failure.map(Into::into),
            emitted_at: value.emitted_at,
        }
    }
}

impl ShellApi {
    pub fn start_provider_model_sync(
        &self,
        connection_id: &str,
        credential: Option<SecretCredential>,
    ) -> ShellResult<ModelSyncStartedDto> {
        validate_identifier("connection_id", connection_id)?;
        self.core
            .start_provider_model_sync(
                &ProviderConnectionId::from(connection_id),
                credential.map(SecretCredential::into_core_value),
            )
            .map(|job_id| ModelSyncStartedDto { job_id: job_id.0 })
            .map_err(ShellError::from)
    }

    /// Starts model synchronization using the exact durable authority paired
    /// with the native bound-credential read. A credential-bound connection
    /// fails closed when the authority is absent or stale; credentialless
    /// connections require no authority.
    pub fn start_provider_model_sync_with_credential_authority(
        &self,
        connection_id: &str,
        credential: Option<SecretCredential>,
        access_authority: Option<ProviderCredentialAccessAuthorityContext>,
    ) -> ShellResult<ModelSyncStartedDto> {
        self.start_provider_model_sync_with_credential_carrier(
            connection_id,
            credential,
            access_authority,
            None,
        )
    }

    /// Starts model synchronization while retaining a native mutation gate
    /// for the complete provider-listing future.
    pub fn start_provider_model_sync_with_credential_authority_and_dispatch_lease(
        &self,
        connection_id: &str,
        credential: Option<SecretCredential>,
        access_authority: Option<ProviderCredentialAccessAuthorityContext>,
        dispatch_lease: TaskCredentialLease,
    ) -> ShellResult<ModelSyncStartedDto> {
        self.start_provider_model_sync_with_credential_carrier(
            connection_id,
            credential,
            access_authority,
            Some(dispatch_lease),
        )
    }

    fn start_provider_model_sync_with_credential_carrier(
        &self,
        connection_id: &str,
        credential: Option<SecretCredential>,
        access_authority: Option<ProviderCredentialAccessAuthorityContext>,
        dispatch_lease: Option<TaskCredentialLease>,
    ) -> ShellResult<ModelSyncStartedDto> {
        validate_identifier("connection_id", connection_id)?;
        let connection_id = ProviderConnectionId::from(connection_id);
        let credential = credential.map(SecretCredential::into_core_value);
        let credential = match access_authority {
            Some(authority) => ConnectionBoundCredential::new_with_access_authority(
                connection_id.clone(),
                credential,
                ProviderCredentialAccessAuthority {
                    authority_id: authority.authority_id,
                    connection_binding_sha256: authority.connection_binding_sha256,
                },
            ),
            None => ConnectionBoundCredential::new(connection_id.clone(), credential),
        };
        let credential = match dispatch_lease {
            Some(lease) => credential.with_dispatch_lease(lease.into_inner()),
            None => credential,
        };
        self.core
            .start_provider_model_sync_with_credential(&connection_id, credential)
            .map(|job_id| ModelSyncStartedDto { job_id: job_id.0 })
            .map_err(ShellError::from)
    }

    pub fn get_provider_model_sync(&self, job_id: &str) -> ShellResult<ModelSyncJobDto> {
        validate_identifier("model_sync_job_id", job_id)?;
        self.core
            .get_provider_model_sync(&ModelSyncJobId::from(job_id))
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn list_provider_model_syncs(
        &self,
        connection_id: &str,
        limit: u32,
    ) -> ShellResult<Vec<ModelSyncJobDto>> {
        validate_identifier("connection_id", connection_id)?;
        self.core
            .list_provider_model_syncs(&ProviderConnectionId::from(connection_id), limit)
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn approve_provider_model_sync(
        &self,
        job_id: &str,
        review_sha256: &str,
    ) -> ShellResult<ModelSyncJobDto> {
        validate_identifier("model_sync_job_id", job_id)?;
        validate_sha256(review_sha256)?;
        self.core
            .approve_provider_model_sync(&ModelSyncJobId::from(job_id), review_sha256)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn cancel_provider_model_sync(&self, job_id: &str) -> ShellResult<ModelSyncJobDto> {
        validate_identifier("model_sync_job_id", job_id)?;
        self.core
            .cancel_provider_model_sync(&ModelSyncJobId::from(job_id))
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn poll_provider_model_sync_events(
        &self,
        job_id: &str,
        limit: u32,
    ) -> ShellResult<Vec<ModelSyncEventDto>> {
        validate_identifier("model_sync_job_id", job_id)?;
        self.core
            .poll_provider_model_sync_events(&ModelSyncJobId::from(job_id), limit)
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn ack_provider_model_sync_event(&self, job_id: &str, sequence: u64) -> ShellResult<bool> {
        validate_identifier("model_sync_job_id", job_id)?;
        self.core
            .ack_provider_model_sync_event(&ModelSyncJobId::from(job_id), sequence)
            .map_err(ShellError::from)
    }
}

fn validate_sha256(value: &str) -> ShellResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(lorepia_core::CoreError::invalid(
            "review_sha256 is not a lowercase SHA-256 digest",
        )
        .into());
    }
    Ok(())
}

const fn model_sync_state_name(value: ModelSyncState) -> &'static str {
    match value {
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

const fn api_family_name(value: ApiFamily) -> &'static str {
    match value {
        ApiFamily::OpenAiResponses => "openai_responses",
        ApiFamily::OpenAiChatCompletions => "openai_chat_completions",
        ApiFamily::AnthropicMessages => "anthropic_messages",
        ApiFamily::GeminiGenerateContent => "gemini_generate_content",
        ApiFamily::OllamaNative => "ollama_native",
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use lorepia_core::{
        ApiFamily, CanonicalOrigin, ConnectionConfig, ConnectionStatus, CredentialRef,
        EndpointPath, MODEL_SYNC_EVENT_VERSION, MODEL_SYNC_REDACTION_VERSION, ModelSyncEvent,
        ModelSyncFailure, ModelSyncJob, ModelSyncJobId, ModelSyncProgress, ModelSyncState,
        ProviderConnection, ProviderConnectionId, ProviderTemplateId,
    };

    use super::{ModelSyncEventDto, ModelSyncJobDto};

    #[test]
    fn model_sync_job_projection_omits_credential_reference() {
        let canary = "credential-ref-model-sync-canary";
        let now = Utc::now();
        let connection = ProviderConnection {
            id: ProviderConnectionId::from("connection"),
            template_id: ProviderTemplateId::from("template"),
            template_version: 1,
            display_name: "Connection".to_owned(),
            api_origin: CanonicalOrigin::parse("https://example.test").expect("origin"),
            config: ConnectionConfig::default(),
            credential_ref: Some(CredentialRef(canary.to_owned())),
            credential_scope: None,
            timeout_seconds: 30,
            status: ConnectionStatus::Untested,
            created_at: now,
            updated_at: now,
        };
        let review = lorepia_core::ModelSyncReview::new(lorepia_core::ModelSyncDiff {
            connection_id: connection.id.clone(),
            expected_connection: connection,
            expected_model_routes: Vec::new(),
            observed_at: now,
            listed_routes: Vec::new(),
            newly_seen_model_route_ids: Vec::new(),
            missing_model_route_ids: Vec::new(),
            initial_presets: Vec::new(),
            capability_observations: Vec::new(),
            routes_requiring_preset_configuration: Vec::new(),
            provenance: lorepia_core::ModelSyncSourceProvenance {
                source: "provider_api".to_owned(),
                api_family: ApiFamily::OpenAiResponses,
                api_origin: CanonicalOrigin::parse("https://example.test").expect("origin"),
                endpoint_path: EndpointPath::parse("/v1/models").expect("path"),
                pages_fetched: 1,
                response_bytes: 128,
            },
        })
        .expect("review");
        let dto = ModelSyncJobDto::from(ModelSyncJob {
            id: ModelSyncJobId::from("job"),
            connection_id: ProviderConnectionId::from("connection"),
            state: ModelSyncState::DiffReadyAwaitingReview,
            revision: 3,
            review: Some(review),
            failure: None,
            created_at: now,
            updated_at: now,
        });
        let json = serde_json::to_string(&dto).expect("serialize");

        assert_eq!(dto.revision, 3);
        assert_eq!(dto.state, "diff-ready-awaiting-review");
        assert!(!json.contains(canary));
        assert!(!json.contains("credential_ref"));
    }

    #[test]
    fn model_sync_event_projection_preserves_durable_identity() {
        let now = Utc::now();
        let dto = ModelSyncEventDto::from(ModelSyncEvent {
            version: MODEL_SYNC_EVENT_VERSION,
            job_id: ModelSyncJobId::from("job"),
            sequence: 7,
            job_revision: 5,
            redaction_version: MODEL_SYNC_REDACTION_VERSION,
            state: ModelSyncState::Failed,
            progress: ModelSyncProgress {
                completed_steps: 1,
                total_steps: 2,
                message_key: "model_sync.failed".to_owned(),
            },
            review_sha256: None,
            failure: Some(ModelSyncFailure {
                code: "provider_unavailable".to_owned(),
                message_key: "model_sync.failed".to_owned(),
                recoverable: true,
            }),
            emitted_at: now,
        });

        assert_eq!(dto.version, MODEL_SYNC_EVENT_VERSION);
        assert_eq!(dto.sequence, 7);
        assert_eq!(dto.job_revision, 5);
        assert_eq!(dto.state, "failed");
        assert_eq!(dto.failure.expect("failure").code, "provider_unavailable");
    }
}
