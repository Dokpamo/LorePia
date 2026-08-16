//! Provider-neutral model generation and an OpenAI-compatible adapter.

mod anthropic_messages;
mod capability_probe;
pub mod catalog;
mod curl_parser;
pub mod discovery;
mod embedding;
mod gemini_generate_content;
mod manifest_validator;
mod network_transport;
mod ollama_native;
mod openai_compatible;
mod openai_responses;
pub mod parameter_mapping;
pub mod prompt_contract;
mod registry;
mod request_plan;
pub mod setup_assistant;
mod sse;
pub mod url_policy;

use async_trait::async_trait;
use lorepia_domain::{
    BoundedJson, CoreResult, GenerationRequest, GenerationUsage, OpaqueReasoningState,
    ProviderCapabilities, ToolCallArgumentsDelta, ToolCallId, ToolName,
};
use tokio::sync::{mpsc, watch};

use crate::parameter_mapping::ProviderRequestPlan;

pub use anthropic_messages::AnthropicMessagesProvider;
pub use capability_probe::{
    AdapterProbeRequest, AdapterProbeResult, CapabilityProbeAdapter, CapabilityProbeEngine,
    CapabilityProbeKind, MergedCapabilityObservation, ProbeAdapterError, ProbeBudget, ProbeConsent,
    ProbeEvidence, ProbeFailure, ProbeRunOutcome, ProbeUsage, ProviderCapabilityProbeAdapter,
    UnknownOutcomeReason, UnknownProbeOutcome, merge_capability_observations,
};
pub use curl_parser::{
    CurlAuthHint, CurlInspection, CurlParseError, CurlParseErrorKind, JsonFieldShape, JsonShape,
    ParsedCurlEvidence, SecretBytes, SecretCurlInput, inspect_curl, parse_curl,
};
pub use embedding::{
    EmbeddingContract, EmbeddingFailure, EmbeddingOutput, EmbeddingProvider, EmbeddingPurpose,
    EmbeddingRequest, EmbeddingRequestSchema, EmbeddingRunOutcome, EmbeddingUnknownOutcome,
    EmbeddingUnknownOutcomeReason, MAX_EMBEDDING_DIMENSIONS, MAX_EMBEDDING_INPUT_BYTES,
    MAX_EMBEDDING_INPUT_CHARS,
};
pub use gemini_generate_content::{GeminiGenerateContentProvider, GeminiResponseMode};
pub use manifest_validator::{
    ValidatedProviderManifest, validate_connection_fields, validate_manifest,
};
pub use ollama_native::{OllamaModelDetails, OllamaModelSummary, OllamaNativeProvider};
pub use openai_compatible::OpenAiCompatibleProvider;
pub use openai_responses::OpenAiResponsesProvider;
pub use prompt_contract::{
    DeveloperRoleCapability, PromptTokenEstimate, ProviderCacheBoundaryArchitecture,
    ProviderCacheBoundaryCompilation, ProviderCacheBoundaryDisposition,
    ProviderCacheBoundaryStrategy, ProviderCacheBoundaryWarning, ProviderCompiledPromptPlan,
    ProviderCompiledPromptPreview, ProviderContextLimit, ProviderPromptAdapterContract,
    ProviderPromptContract, ProviderPromptContractError, ProviderPromptMessage,
    ProviderPromptMessagePreview, ProviderPromptPlacement, ProviderPromptTokenizer,
    ProviderRoleMapping, ProviderRoleMappingReason, ProviderWireRole,
};
pub use registry::{
    AdapterDescriptor, AdapterRegistry, BuiltInTemplateId, ListedModel, ListedModelCapabilities,
    ListedModelCapability, ListedModelReasoningCapability, ModelListBudget, ModelListProvenance,
    ModelListRequest, ModelListResult, ModelListSupport, ModelListing, ModelRecordSource,
    OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR, OpenRouterReasoningEffort,
    OpenRouterReasoningEffortSupport, OpenRouterSupportedParameter,
    OpenRouterSupportedParameterSupport,
};
pub use request_plan::{
    RequestBodyField, RequestBodyShape, RequestPreview, RequestPreviewError,
    RequestPreviewErrorKind, build_request_preview,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    TextDelta(String),
    ReasoningDelta(String),
    /// Provider-native continuity state for the next request.
    ///
    /// Chat consumes this internally and never exposes it as a `ChatEvent`.
    OpaqueReasoningState(OpaqueReasoningState),
    /// Declares one inert provider-requested tool call.
    ///
    /// `LorePia` does not execute calls from this event. The subsequent argument
    /// fragments remain opaque until a future, separately authorized tool
    /// runtime consumes the completed protocol state.
    ToolCallStarted {
        id: ToolCallId,
        name: ToolName,
    },
    ToolCallArgumentsDelta {
        id: ToolCallId,
        delta: ToolCallArgumentsDelta,
    },
    ToolCallCompleted {
        id: ToolCallId,
    },
}

pub type ProviderEventSender = mpsc::Sender<ProviderEvent>;

const OPAQUE_REQUEST_SNAPSHOT_UNSUPPORTED_ERROR: &str =
    "opaque reasoning continuity cannot be stored in a plaintext request snapshot";

fn validate_snapshot_request(request: &GenerationRequest) -> CoreResult<()> {
    if request.preserve_opaque_reasoning_state || !request.opaque_reasoning_context.is_empty() {
        return Err(lorepia_domain::CoreError::new(
            lorepia_domain::CoreErrorCode::UnsupportedContent,
            OPAQUE_REQUEST_SNAPSHOT_UNSUPPORTED_ERROR,
            false,
        ));
    }
    Ok(())
}

/// Merge recognized numeric usage counters into a bounded provider-native
/// summary. Callers supply static field names and never pass through an
/// arbitrary response object or provider-controlled string.
pub(crate) fn merge_usage_summary(
    current: Option<&BoundedJson>,
    fields: &[(&'static str, Option<u64>)],
) -> Option<BoundedJson> {
    let mut object = current
        .and_then(|summary| serde_json::from_str::<serde_json::Value>(summary.as_str()).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    for (name, value) in fields {
        if let Some(value) = value {
            object.insert((*name).to_owned(), serde_json::Value::from(*value));
        }
    }
    if object.is_empty() {
        None
    } else {
        BoundedJson::from_value(&serde_json::Value::Object(object)).ok()
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn capabilities(&self) -> ProviderCapabilities;

    /// Builds the exact credential-free JSON request body which this adapter
    /// would dispatch for `request`.
    ///
    /// The snapshot contains no endpoint, headers, or credential. Opaque
    /// provider continuity is rejected because its plaintext topology must not
    /// be persisted by orchestration storage.
    fn snapshot_request(&self, _request: &GenerationRequest) -> CoreResult<serde_json::Value> {
        Err(lorepia_domain::CoreError::new(
            lorepia_domain::CoreErrorCode::UnsupportedContent,
            "the compiled provider does not support request snapshots",
            false,
        ))
    }

    async fn generate(
        &self,
        request: GenerationRequest,
        credential: Option<&str>,
        sink: ProviderEventSender,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage>;

    /// Executes a generation with a closed, Rust-authored internal request
    /// plan.
    ///
    /// The default fails closed. Built-in adapters override this so trusted
    /// Rust workflows can add fixed schema/tool contracts without exposing a
    /// forgeable raw-body API to Core or native callers.
    #[doc(hidden)]
    async fn generate_with_internal_plan(
        &self,
        _request: GenerationRequest,
        _credential: Option<&str>,
        _sink: ProviderEventSender,
        _cancelled: watch::Receiver<bool>,
        _request_plan: ProviderRequestPlan,
    ) -> CoreResult<GenerationUsage> {
        Err(lorepia_domain::CoreError::new(
            lorepia_domain::CoreErrorCode::UnsupportedContent,
            "the compiled provider does not support internal request plans",
            false,
        ))
    }
}

/// Deterministic provider used by unit tests and offline previews.
pub struct StaticProvider {
    response: String,
}

impl StaticProvider {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
        }
    }
}

#[async_trait]
impl Provider for StaticProvider {
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
        sink: ProviderEventSender,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        if *cancelled.borrow() {
            return Err(lorepia_domain::CoreError::new(
                lorepia_domain::CoreErrorCode::Cancelled,
                "generation was cancelled",
                true,
            ));
        }
        sink.send(ProviderEvent::TextDelta(self.response.clone()))
            .await
            .map_err(|_| lorepia_domain::CoreError::internal("provider event receiver closed"))?;
        Ok(GenerationUsage {
            input_tokens: None,
            output_tokens: None,
            ..GenerationUsage::default()
        })
    }
}
