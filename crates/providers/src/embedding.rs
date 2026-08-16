use std::fmt;

use async_trait::async_trait;
use futures_util::StreamExt;
use lorepia_domain::{
    ApiFamily, AuthBinding, CanonicalOrigin, CoreError, CoreErrorCode, CoreResult, EndpointPath,
    ModelRouteId, ProviderConnectionId,
};
use reqwest::{
    Response, StatusCode,
    header::{ACCEPT, CONTENT_TYPE},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use zeroize::Zeroizing;

use crate::network_transport::{
    PreparedHttpTarget, ProviderHttpTarget, authorize_request, validate_credential_for_auth,
};

pub const MAX_EMBEDDING_DIMENSIONS: u32 = 32_768;
pub const MAX_EMBEDDING_INPUT_BYTES: usize = 8 * 1024;
pub const MAX_EMBEDDING_INPUT_CHARS: usize = 8 * 1024;

const MAX_EMBEDDING_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// The closed provider hint used to keep document and query vectors in the
/// same provider-native retrieval space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddingPurpose {
    RetrievalDocument,
    RetrievalQuery,
}

impl EmbeddingPurpose {
    const fn digest_name(self) -> &'static str {
        match self {
            Self::RetrievalDocument => "retrieval_document",
            Self::RetrievalQuery => "retrieval_query",
        }
    }

    const fn gemini_task_type(self) -> &'static str {
        match self {
            Self::RetrievalDocument => "RETRIEVAL_DOCUMENT",
            Self::RetrievalQuery => "RETRIEVAL_QUERY",
        }
    }
}

/// One bounded embedding input.
///
/// The input is deliberately neither `Debug` nor `Serialize`. It is owned by a
/// zeroizing buffer until the closed adapter materializes the provider request.
pub struct EmbeddingRequest {
    model: String,
    input: Zeroizing<String>,
    dimensions: u32,
    purpose: EmbeddingPurpose,
}

impl EmbeddingRequest {
    pub fn new(
        model: impl Into<String>,
        input: impl Into<String>,
        dimensions: u32,
        purpose: EmbeddingPurpose,
    ) -> CoreResult<Self> {
        let model = model.into();
        validate_request_model(&model)?;
        validate_dimensions(dimensions)?;
        let input = Zeroizing::new(input.into());
        validate_input(&input)?;
        Ok(Self {
            model,
            input,
            dimensions,
            purpose,
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub const fn dimensions(&self) -> u32 {
        self.dimensions
    }

    pub const fn purpose(&self) -> EmbeddingPurpose {
        self.purpose
    }

    fn into_input(self) -> Zeroizing<String> {
        self.input
    }
}

/// A validated vector. Debug output exposes only its dimensionality.
#[derive(Clone, PartialEq)]
pub struct EmbeddingOutput {
    values: Vec<f32>,
}

impl EmbeddingOutput {
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    pub fn into_values(self) -> Vec<f32> {
        self.values
    }
}

impl fmt::Debug for EmbeddingOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EmbeddingOutput")
            .field("dimensions", &self.values.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddingRequestSchema {
    OpenAiEmbeddingsV1,
    OpenRouterEmbeddingsV1,
    GeminiEmbedContentV1,
    OllamaEmbedV1,
}

impl EmbeddingRequestSchema {
    const fn digest_name(self) -> &'static str {
        match self {
            Self::OpenAiEmbeddingsV1 => "openai_embeddings_v1",
            Self::OpenRouterEmbeddingsV1 => "openrouter_embeddings_v1",
            Self::GeminiEmbedContentV1 => "gemini_embed_content_v1",
            Self::OllamaEmbedV1 => "ollama_embed_v1",
        }
    }
}

/// Exact secret-free destination/model contract fixed before a worker may
/// obtain a credential.
#[derive(Clone, PartialEq, Eq)]
pub struct EmbeddingContract {
    connection_id: ProviderConnectionId,
    model_route_id: ModelRouteId,
    api_family: ApiFamily,
    api_origin: CanonicalOrigin,
    model_id: String,
    dimensions: u32,
    endpoint_path: EndpointPath,
    manifest_sha256: String,
    request_schema: EmbeddingRequestSchema,
}

impl EmbeddingContract {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        connection_id: ProviderConnectionId,
        model_route_id: ModelRouteId,
        api_family: ApiFamily,
        api_origin: CanonicalOrigin,
        model_id: String,
        dimensions: u32,
        endpoint_path: EndpointPath,
        manifest_sha256: String,
        request_schema: EmbeddingRequestSchema,
    ) -> CoreResult<Self> {
        validate_request_model(&model_id)?;
        validate_dimensions(dimensions)?;
        if manifest_sha256.len() != 64
            || !manifest_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(CoreError::internal(
                "validated provider manifest has an invalid digest",
            ));
        }
        Ok(Self {
            connection_id,
            model_route_id,
            api_family,
            api_origin,
            model_id,
            dimensions,
            endpoint_path,
            manifest_sha256,
            request_schema,
        })
    }

    pub fn connection_id(&self) -> &ProviderConnectionId {
        &self.connection_id
    }

    pub fn model_route_id(&self) -> &ModelRouteId {
        &self.model_route_id
    }

    pub const fn api_family(&self) -> ApiFamily {
        self.api_family
    }

    pub fn api_origin(&self) -> &CanonicalOrigin {
        &self.api_origin
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub const fn dimensions(&self) -> u32 {
        self.dimensions
    }

    pub fn endpoint_path(&self) -> &EndpointPath {
        &self.endpoint_path
    }

    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub const fn request_schema(&self) -> EmbeddingRequestSchema {
        self.request_schema
    }

    /// Hashes every field which can change the stored vector space.
    ///
    /// Retrieval document/query purpose is intentionally excluded so both
    /// sides of one retrieval space share this identity. The digest still
    /// binds the exact connection, route, origin, API family, model,
    /// dimensions, endpoint, manifest and closed request schema.
    pub fn vector_space_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest_component(&mut digest, b"lorepia-embedding-vector-space-v1");
        digest_component(&mut digest, self.connection_id.as_str().as_bytes());
        digest_component(&mut digest, self.model_route_id.as_str().as_bytes());
        digest_component(&mut digest, api_family_name(self.api_family).as_bytes());
        digest_component(&mut digest, self.api_origin.as_str().as_bytes());
        digest_component(&mut digest, self.model_id.as_bytes());
        digest_component(&mut digest, &self.dimensions.to_be_bytes());
        digest_component(&mut digest, self.endpoint_path.as_str().as_bytes());
        digest_component(&mut digest, self.manifest_sha256.as_bytes());
        digest_component(&mut digest, self.request_schema.digest_name().as_bytes());
        format!("{:x}", digest.finalize())
    }

    /// Binds one document/query dispatch to its exact vector space.
    pub fn execution_sha256(&self, purpose: EmbeddingPurpose) -> String {
        let mut digest = Sha256::new();
        digest_component(&mut digest, b"lorepia-embedding-execution-v1");
        digest_component(&mut digest, self.vector_space_sha256().as_bytes());
        digest_component(&mut digest, purpose.digest_name().as_bytes());
        format!("{:x}", digest.finalize())
    }
}

impl fmt::Debug for EmbeddingContract {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EmbeddingContract")
            .field("connection_id", &self.connection_id)
            .field("model_route_id", &self.model_route_id)
            .field("api_family", &self.api_family)
            .field("api_origin", &self.api_origin)
            .field("model_id", &self.model_id)
            .field("dimensions", &self.dimensions)
            .field("endpoint_path", &self.endpoint_path)
            .field("manifest_sha256", &self.manifest_sha256)
            .field("request_schema", &self.request_schema)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingFailure {
    InvalidRequest,
    Authentication,
    RateLimited,
    ProviderRejected,
    ProviderUnavailable,
    ProtocolViolation,
}

impl EmbeddingFailure {
    pub const fn recoverable(self) -> bool {
        matches!(self, Self::RateLimited | Self::ProviderUnavailable)
    }

    pub fn into_core_error(self) -> CoreError {
        let (code, message) = match self {
            Self::InvalidRequest => (
                CoreErrorCode::InvalidInput,
                "embedding request does not match its resolved provider contract",
            ),
            Self::Authentication => (
                CoreErrorCode::ProviderAuthFailed,
                "embedding provider rejected authentication",
            ),
            Self::RateLimited => (
                CoreErrorCode::ProviderRateLimited,
                "embedding provider rate limited the request",
            ),
            Self::ProviderRejected => (
                CoreErrorCode::UnsupportedContent,
                "embedding provider rejected the request",
            ),
            Self::ProviderUnavailable => (
                CoreErrorCode::ProviderUnavailable,
                "embedding provider is unavailable",
            ),
            Self::ProtocolViolation => (
                CoreErrorCode::ProviderUnavailable,
                "embedding provider returned an invalid response",
            ),
        };
        CoreError::new(code, message, self.recoverable())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingUnknownOutcomeReason {
    CancelledAfterDispatch,
    TimedOutAfterDispatch,
    TransportInterruptedAfterDispatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingUnknownOutcome {
    reason: EmbeddingUnknownOutcomeReason,
    contract_sha256: String,
}

impl EmbeddingUnknownOutcome {
    pub const fn reason(&self) -> EmbeddingUnknownOutcomeReason {
        self.reason
    }

    pub fn contract_sha256(&self) -> &str {
        &self.contract_sha256
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EmbeddingRunOutcome {
    Completed(EmbeddingOutput),
    Failed(EmbeddingFailure),
    CancelledBeforeDispatch,
    UnknownOutcome(EmbeddingUnknownOutcome),
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn contract(&self) -> &EmbeddingContract;

    async fn embed(
        &self,
        request: EmbeddingRequest,
        credential: Option<&str>,
        cancelled: watch::Receiver<bool>,
    ) -> EmbeddingRunOutcome;
}

#[derive(Clone)]
pub(crate) struct HttpEmbeddingProvider {
    contract: EmbeddingContract,
    endpoint: url::Url,
    target: ProviderHttpTarget,
    auth: AuthBinding,
}

impl HttpEmbeddingProvider {
    pub(crate) fn new(
        contract: EmbeddingContract,
        target: ProviderHttpTarget,
        auth: AuthBinding,
    ) -> Self {
        Self {
            endpoint: target.url().clone(),
            contract,
            target,
            auth,
        }
    }

    fn request_body(&self, request: EmbeddingRequest) -> Value {
        let purpose = request.purpose;
        let input = request.into_input();
        match self.contract.request_schema {
            EmbeddingRequestSchema::OpenAiEmbeddingsV1 => json!({
                "model": self.contract.model_id,
                "input": input.as_str(),
                "dimensions": self.contract.dimensions,
                "encoding_format": "float",
            }),
            EmbeddingRequestSchema::OpenRouterEmbeddingsV1 => json!({
                "model": self.contract.model_id,
                "input": input.as_str(),
                "dimensions": self.contract.dimensions,
                "encoding_format": "float",
                "provider": {
                    "allow_fallbacks": false,
                    "require_parameters": true,
                },
            }),
            EmbeddingRequestSchema::GeminiEmbedContentV1 => json!({
                "model": format!("models/{}", self.contract.model_id),
                "content": {
                    "parts": [{"text": input.as_str()}],
                },
                "embedContentConfig": {
                    "taskType": purpose.gemini_task_type(),
                    "autoTruncate": false,
                    "outputDimensionality": self.contract.dimensions,
                },
            }),
            EmbeddingRequestSchema::OllamaEmbedV1 => json!({
                "model": self.contract.model_id,
                "input": input.as_str(),
                "truncate": false,
                "dimensions": self.contract.dimensions,
            }),
        }
    }

    fn failed_from_core(error: &CoreError) -> EmbeddingFailure {
        match error.code {
            CoreErrorCode::ProviderAuthFailed => EmbeddingFailure::Authentication,
            CoreErrorCode::ProviderRateLimited => EmbeddingFailure::RateLimited,
            CoreErrorCode::InvalidInput
            | CoreErrorCode::UnsupportedContent
            | CoreErrorCode::PermissionDenied
            | CoreErrorCode::Cancelled => EmbeddingFailure::InvalidRequest,
            CoreErrorCode::NetworkUnavailable | CoreErrorCode::ProviderUnavailable => {
                EmbeddingFailure::ProviderUnavailable
            }
            CoreErrorCode::UnsafeArchive
            | CoreErrorCode::NotFound
            | CoreErrorCode::StorageUnavailable
            | CoreErrorCode::StorageCorrupted
            | CoreErrorCode::Internal => EmbeddingFailure::ProtocolViolation,
        }
    }

    fn unknown(
        &self,
        purpose: EmbeddingPurpose,
        reason: EmbeddingUnknownOutcomeReason,
    ) -> EmbeddingRunOutcome {
        EmbeddingRunOutcome::UnknownOutcome(EmbeddingUnknownOutcome {
            reason,
            contract_sha256: self.contract.execution_sha256(purpose),
        })
    }

    async fn consume_response(
        &self,
        prepared: &PreparedHttpTarget,
        response: Response,
        purpose: EmbeddingPurpose,
        cancelled: &mut watch::Receiver<bool>,
    ) -> EmbeddingRunOutcome {
        if let Err(error) = prepared.validate_response_peer(&response) {
            return EmbeddingRunOutcome::Failed(Self::failed_from_core(&error));
        }
        if !response.status().is_success() {
            return EmbeddingRunOutcome::Failed(failure_for_status(response.status()));
        }
        if !is_json_response(&response)
            || response
                .content_length()
                .is_some_and(|length| length > MAX_EMBEDDING_RESPONSE_BYTES as u64)
        {
            return EmbeddingRunOutcome::Failed(EmbeddingFailure::ProtocolViolation);
        }

        let body = match collect_response(response, cancelled).await {
            BodyOutcome::Complete(body) => body,
            BodyOutcome::TooLarge => {
                return EmbeddingRunOutcome::Failed(EmbeddingFailure::ProtocolViolation);
            }
            BodyOutcome::Cancelled => {
                return self.unknown(
                    purpose,
                    EmbeddingUnknownOutcomeReason::CancelledAfterDispatch,
                );
            }
            BodyOutcome::TimedOut => {
                return self.unknown(
                    purpose,
                    EmbeddingUnknownOutcomeReason::TimedOutAfterDispatch,
                );
            }
            BodyOutcome::Interrupted => {
                return self.unknown(
                    purpose,
                    EmbeddingUnknownOutcomeReason::TransportInterruptedAfterDispatch,
                );
            }
        };
        match parse_embedding(
            self.contract.request_schema,
            &body,
            self.contract.dimensions,
            &self.contract.model_id,
        ) {
            Ok(output) => EmbeddingRunOutcome::Completed(output),
            Err(failure) => EmbeddingRunOutcome::Failed(failure),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for HttpEmbeddingProvider {
    fn contract(&self) -> &EmbeddingContract {
        &self.contract
    }

    async fn embed(
        &self,
        request: EmbeddingRequest,
        credential: Option<&str>,
        mut cancelled: watch::Receiver<bool>,
    ) -> EmbeddingRunOutcome {
        if *cancelled.borrow() {
            return EmbeddingRunOutcome::CancelledBeforeDispatch;
        }
        if request.model != self.contract.model_id || request.dimensions != self.contract.dimensions
        {
            return EmbeddingRunOutcome::Failed(EmbeddingFailure::InvalidRequest);
        }
        if let Some(credential) = credential.filter(|value| !value.is_empty())
            && request.input.contains(credential)
        {
            return EmbeddingRunOutcome::Failed(EmbeddingFailure::InvalidRequest);
        }
        if let Err(error) = validate_credential_for_auth(&self.auth, credential) {
            return EmbeddingRunOutcome::Failed(Self::failed_from_core(&error));
        }

        let purpose = request.purpose;
        let prepared = match prepare_before_dispatch(&self.target, &mut cancelled).await {
            PrepareOutcome::Prepared(prepared) => prepared,
            PrepareOutcome::Cancelled => return EmbeddingRunOutcome::CancelledBeforeDispatch,
            PrepareOutcome::Failed(error) => {
                return EmbeddingRunOutcome::Failed(Self::failed_from_core(&error));
            }
        };
        if *cancelled.borrow() {
            return EmbeddingRunOutcome::CancelledBeforeDispatch;
        }

        let body = self.request_body(request);
        let request = match authorize_request(
            prepared
                .client()
                .post(self.endpoint.clone())
                .header(ACCEPT, "application/json")
                .header(CONTENT_TYPE, "application/json")
                .json(&body),
            &self.auth,
            credential,
        ) {
            Ok(request) => request,
            Err(error) => {
                return EmbeddingRunOutcome::Failed(Self::failed_from_core(&error));
            }
        };

        let response = match send_after_dispatch(request, &mut cancelled).await {
            DispatchOutcome::Response(response) => response,
            DispatchOutcome::Cancelled => {
                return self.unknown(
                    purpose,
                    EmbeddingUnknownOutcomeReason::CancelledAfterDispatch,
                );
            }
            DispatchOutcome::TimedOut => {
                return self.unknown(
                    purpose,
                    EmbeddingUnknownOutcomeReason::TimedOutAfterDispatch,
                );
            }
            DispatchOutcome::Interrupted => {
                return self.unknown(
                    purpose,
                    EmbeddingUnknownOutcomeReason::TransportInterruptedAfterDispatch,
                );
            }
        };

        self.consume_response(&prepared, response, purpose, &mut cancelled)
            .await
    }
}

enum PrepareOutcome {
    Prepared(Box<PreparedHttpTarget>),
    Cancelled,
    Failed(CoreError),
}

async fn prepare_before_dispatch(
    target: &ProviderHttpTarget,
    cancelled: &mut watch::Receiver<bool>,
) -> PrepareOutcome {
    let operation = target.prepare();
    tokio::pin!(operation);
    let mut cancellation_open = true;
    loop {
        tokio::select! {
            biased;
            change = cancelled.changed(), if cancellation_open => {
                match change {
                    Ok(()) if *cancelled.borrow() => return PrepareOutcome::Cancelled,
                    Ok(()) => {}
                    Err(_) => cancellation_open = false,
                }
            }
            result = &mut operation => {
                return match result {
                    Ok(prepared) => PrepareOutcome::Prepared(Box::new(prepared)),
                    Err(error) => PrepareOutcome::Failed(error),
                };
            }
        }
    }
}

enum DispatchOutcome {
    Response(Response),
    Cancelled,
    TimedOut,
    Interrupted,
}

async fn send_after_dispatch(
    request: reqwest::RequestBuilder,
    cancelled: &mut watch::Receiver<bool>,
) -> DispatchOutcome {
    let operation = request.send();
    tokio::pin!(operation);
    let mut cancellation_open = true;
    loop {
        tokio::select! {
            biased;
            change = cancelled.changed(), if cancellation_open => {
                match change {
                    Ok(()) if *cancelled.borrow() => return DispatchOutcome::Cancelled,
                    Ok(()) => {}
                    Err(_) => cancellation_open = false,
                }
            }
            result = &mut operation => {
                return match result {
                    Ok(response) => DispatchOutcome::Response(response),
                    Err(error) if error.is_timeout() => DispatchOutcome::TimedOut,
                    Err(_) => DispatchOutcome::Interrupted,
                };
            }
        }
    }
}

enum BodyOutcome {
    Complete(Vec<u8>),
    TooLarge,
    Cancelled,
    TimedOut,
    Interrupted,
}

async fn collect_response(
    response: Response,
    cancelled: &mut watch::Receiver<bool>,
) -> BodyOutcome {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    let mut cancellation_open = true;
    loop {
        tokio::select! {
            biased;
            change = cancelled.changed(), if cancellation_open => {
                match change {
                    Ok(()) if *cancelled.borrow() => return BodyOutcome::Cancelled,
                    Ok(()) => {}
                    Err(_) => cancellation_open = false,
                }
            }
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(chunk)) => {
                        let Some(next_size) = body.len().checked_add(chunk.len()) else {
                            return BodyOutcome::TooLarge;
                        };
                        if next_size > MAX_EMBEDDING_RESPONSE_BYTES {
                            return BodyOutcome::TooLarge;
                        }
                        body.extend_from_slice(&chunk);
                    }
                    Some(Err(error)) if error.is_timeout() => return BodyOutcome::TimedOut,
                    Some(Err(_)) => return BodyOutcome::Interrupted,
                    None => return BodyOutcome::Complete(body),
                }
            }
        }
    }
}

fn failure_for_status(status: StatusCode) -> EmbeddingFailure {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => EmbeddingFailure::Authentication,
        StatusCode::TOO_MANY_REQUESTS => EmbeddingFailure::RateLimited,
        _ if status.is_client_error() => EmbeddingFailure::ProviderRejected,
        _ => EmbeddingFailure::ProviderUnavailable,
    }
}

fn is_json_response(response: &Response) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| {
            let media_type = media_type.trim();
            media_type.eq_ignore_ascii_case("application/json")
                || media_type
                    .strip_prefix("application/")
                    .is_some_and(|subtype| subtype.ends_with("+json"))
        })
}

fn parse_embedding(
    schema: EmbeddingRequestSchema,
    body: &[u8],
    dimensions: u32,
    expected_model: &str,
) -> Result<EmbeddingOutput, EmbeddingFailure> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| EmbeddingFailure::ProtocolViolation)?;
    let vector = match schema {
        EmbeddingRequestSchema::OpenAiEmbeddingsV1
        | EmbeddingRequestSchema::OpenRouterEmbeddingsV1 => {
            if value.get("model").and_then(Value::as_str) != Some(expected_model) {
                return Err(EmbeddingFailure::ProtocolViolation);
            }
            let data = value
                .get("data")
                .and_then(Value::as_array)
                .ok_or(EmbeddingFailure::ProtocolViolation)?;
            if data.len() != 1 {
                return Err(EmbeddingFailure::ProtocolViolation);
            }
            data[0]
                .get("embedding")
                .and_then(Value::as_array)
                .ok_or(EmbeddingFailure::ProtocolViolation)?
        }
        EmbeddingRequestSchema::GeminiEmbedContentV1 => value
            .get("embedding")
            .and_then(|embedding| embedding.get("values"))
            .and_then(Value::as_array)
            .ok_or(EmbeddingFailure::ProtocolViolation)?,
        EmbeddingRequestSchema::OllamaEmbedV1 => {
            if value.get("model").and_then(Value::as_str) != Some(expected_model) {
                return Err(EmbeddingFailure::ProtocolViolation);
            }
            let embeddings = value
                .get("embeddings")
                .and_then(Value::as_array)
                .ok_or(EmbeddingFailure::ProtocolViolation)?;
            if embeddings.len() != 1 {
                return Err(EmbeddingFailure::ProtocolViolation);
            }
            embeddings[0]
                .as_array()
                .ok_or(EmbeddingFailure::ProtocolViolation)?
        }
    };

    let expected = usize::try_from(dimensions).map_err(|_| EmbeddingFailure::ProtocolViolation)?;
    if vector.len() != expected {
        return Err(EmbeddingFailure::ProtocolViolation);
    }
    let mut values = Vec::with_capacity(expected);
    let mut has_nonzero = false;
    for value in vector {
        let value = serde_json::from_value::<f32>(value.clone())
            .map_err(|_| EmbeddingFailure::ProtocolViolation)?;
        if !value.is_finite() {
            return Err(EmbeddingFailure::ProtocolViolation);
        }
        has_nonzero |= value != 0.0;
        values.push(value);
    }
    if !has_nonzero {
        return Err(EmbeddingFailure::ProtocolViolation);
    }
    Ok(EmbeddingOutput { values })
}

fn validate_request_model(model: &str) -> CoreResult<()> {
    if model.is_empty()
        || model.trim() != model
        || model.len() > 1024
        || model.chars().count() > 256
        || model.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(
            "embedding request has an invalid model identifier",
        ));
    }
    Ok(())
}

fn validate_dimensions(dimensions: u32) -> CoreResult<()> {
    if !(1..=MAX_EMBEDDING_DIMENSIONS).contains(&dimensions) {
        return Err(CoreError::invalid(format!(
            "embedding dimensions must be from 1 to {MAX_EMBEDDING_DIMENSIONS}"
        )));
    }
    Ok(())
}

fn validate_input(input: &str) -> CoreResult<()> {
    if input.is_empty()
        || input.len() > MAX_EMBEDDING_INPUT_BYTES
        || input.chars().count() > MAX_EMBEDDING_INPUT_CHARS
        || input.contains('\0')
    {
        return Err(CoreError::invalid(
            "embedding input is empty or exceeds the bounded text contract",
        ));
    }
    Ok(())
}

fn digest_component(digest: &mut Sha256, value: &[u8]) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value);
}

const fn api_family_name(family: ApiFamily) -> &'static str {
    match family {
        ApiFamily::OpenAiResponses => "openai_responses",
        ApiFamily::OpenAiChatCompletions => "openai_chat_completions",
        ApiFamily::AnthropicMessages => "anthropic_messages",
        ApiFamily::GeminiGenerateContent => "gemini_generate_content",
        ApiFamily::OllamaNative => "ollama_native",
    }
}

#[cfg(test)]
mod tests {
    use lorepia_domain::{
        ApiFamily, CanonicalOrigin, EndpointPath, ModelRouteId, ProviderConnectionId,
    };

    use super::{
        EmbeddingContract, EmbeddingFailure, EmbeddingPurpose, EmbeddingRequest,
        EmbeddingRequestSchema, EmbeddingRunOutcome, MAX_EMBEDDING_DIMENSIONS, parse_embedding,
    };

    fn contract(
        connection_id: &str,
        route_id: &str,
        origin: &str,
        model: &str,
        dimensions: u32,
        manifest_byte: char,
        schema: EmbeddingRequestSchema,
    ) -> EmbeddingContract {
        EmbeddingContract::new(
            ProviderConnectionId::from(connection_id),
            ModelRouteId::from(route_id),
            ApiFamily::OpenAiResponses,
            CanonicalOrigin::parse(origin).expect("canonical origin"),
            model.to_owned(),
            dimensions,
            EndpointPath::parse("/v1/embeddings").expect("endpoint path"),
            manifest_byte.to_string().repeat(64),
            schema,
        )
        .expect("embedding contract")
    }

    #[test]
    fn request_bounds_are_enforced_before_dispatch() {
        assert!(
            EmbeddingRequest::new("model", "text", 0, EmbeddingPurpose::RetrievalDocument).is_err()
        );
        assert!(
            EmbeddingRequest::new(
                "model",
                "text",
                MAX_EMBEDDING_DIMENSIONS + 1,
                EmbeddingPurpose::RetrievalDocument
            )
            .is_err()
        );
        assert!(EmbeddingRequest::new("model", "", 3, EmbeddingPurpose::RetrievalQuery).is_err());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn vector_space_digest_excludes_purpose_and_binds_every_mutable_contract_axis() {
        let base = contract(
            "connection",
            "route",
            "https://api.example.test",
            "model",
            3,
            'a',
            EmbeddingRequestSchema::OpenAiEmbeddingsV1,
        );
        let vector_space = base.vector_space_sha256();
        assert_eq!(vector_space.len(), 64);
        assert_ne!(
            base.execution_sha256(EmbeddingPurpose::RetrievalDocument),
            base.execution_sha256(EmbeddingPurpose::RetrievalQuery)
        );

        let mutations = [
            contract(
                "other-connection",
                "route",
                "https://api.example.test",
                "model",
                3,
                'a',
                EmbeddingRequestSchema::OpenAiEmbeddingsV1,
            ),
            contract(
                "connection",
                "other-route",
                "https://api.example.test",
                "model",
                3,
                'a',
                EmbeddingRequestSchema::OpenAiEmbeddingsV1,
            ),
            contract(
                "connection",
                "route",
                "https://other.example.test",
                "model",
                3,
                'a',
                EmbeddingRequestSchema::OpenAiEmbeddingsV1,
            ),
            contract(
                "connection",
                "route",
                "https://api.example.test",
                "other-model",
                3,
                'a',
                EmbeddingRequestSchema::OpenAiEmbeddingsV1,
            ),
            contract(
                "connection",
                "route",
                "https://api.example.test",
                "model",
                4,
                'a',
                EmbeddingRequestSchema::OpenAiEmbeddingsV1,
            ),
            contract(
                "connection",
                "route",
                "https://api.example.test",
                "model",
                3,
                'b',
                EmbeddingRequestSchema::OpenAiEmbeddingsV1,
            ),
            contract(
                "connection",
                "route",
                "https://api.example.test",
                "model",
                3,
                'a',
                EmbeddingRequestSchema::OpenRouterEmbeddingsV1,
            ),
            EmbeddingContract::new(
                ProviderConnectionId::from("connection"),
                ModelRouteId::from("route"),
                ApiFamily::OpenAiChatCompletions,
                CanonicalOrigin::parse("https://api.example.test").expect("canonical origin"),
                "model".to_owned(),
                3,
                EndpointPath::parse("/v1/embeddings").expect("endpoint path"),
                "a".repeat(64),
                EmbeddingRequestSchema::OpenAiEmbeddingsV1,
            )
            .expect("API-family mutation"),
            EmbeddingContract::new(
                ProviderConnectionId::from("connection"),
                ModelRouteId::from("route"),
                ApiFamily::OpenAiResponses,
                CanonicalOrigin::parse("https://api.example.test").expect("canonical origin"),
                "model".to_owned(),
                3,
                EndpointPath::parse("/v1/alternate-embeddings").expect("endpoint path"),
                "a".repeat(64),
                EmbeddingRequestSchema::OpenAiEmbeddingsV1,
            )
            .expect("endpoint mutation"),
        ];
        for mutation in mutations {
            assert_ne!(mutation.vector_space_sha256(), vector_space);
        }
    }

    #[test]
    fn response_topology_dimension_finiteness_and_norm_are_exact() {
        let valid = br#"{"model":"model","data":[{"embedding":[0.25,-0.5,0.75]}]}"#;
        let output = parse_embedding(
            EmbeddingRequestSchema::OpenAiEmbeddingsV1,
            valid,
            3,
            "model",
        )
        .expect("valid vector");
        assert_eq!(output.values(), &[0.25, -0.5, 0.75]);

        for invalid in [
            br#"{"model":"other","data":[{"embedding":[1,2,3]}]}"#.as_slice(),
            br#"{"data":[]}"#.as_slice(),
            br#"{"data":[{"embedding":[1,2,3]},{"embedding":[1,2,3]}]}"#.as_slice(),
            br#"{"data":[{"embedding":[1,2]}]}"#.as_slice(),
            br#"{"data":[{"embedding":[0,0,0]}]}"#.as_slice(),
            br#"{"data":[{"embedding":[1e1000,2,3]}]}"#.as_slice(),
        ] {
            assert_eq!(
                parse_embedding(
                    EmbeddingRequestSchema::OpenAiEmbeddingsV1,
                    invalid,
                    3,
                    "model",
                ),
                Err(EmbeddingFailure::ProtocolViolation)
            );
        }
    }

    #[test]
    fn all_closed_response_schemas_extract_one_vector() {
        let gemini = parse_embedding(
            EmbeddingRequestSchema::GeminiEmbedContentV1,
            br#"{"embedding":{"values":[1,2,3]}}"#,
            3,
            "model",
        )
        .expect("Gemini vector");
        let ollama = parse_embedding(
            EmbeddingRequestSchema::OllamaEmbedV1,
            br#"{"model":"model","embeddings":[[1,2,3]]}"#,
            3,
            "model",
        )
        .expect("Ollama vector");
        assert_eq!(gemini.values(), ollama.values());
    }

    #[test]
    fn output_debug_never_exposes_vector_values() {
        let outcome = EmbeddingRunOutcome::Completed(
            parse_embedding(
                EmbeddingRequestSchema::OllamaEmbedV1,
                br#"{"model":"model","embeddings":[[0.1234567,2,3]]}"#,
                3,
                "model",
            )
            .expect("vector"),
        );
        let debug = format!("{outcome:?}");
        assert!(debug.contains("dimensions"));
        assert!(!debug.contains("0.1234567"));
    }
}
