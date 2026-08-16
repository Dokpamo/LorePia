use std::{collections::HashSet, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures_util::StreamExt;
use lorepia_domain::{
    ApiFamily, AuthBinding, CanonicalOrigin, ConnectionFieldSpec, ConnectionFieldType, CoreError,
    CoreErrorCode, CoreResult, CredentialRedirectPolicy, DecoderId, EndpointPath, EndpointSpec,
    HeaderName as DomainHeaderName, HttpMethod, HttpUrl, ManifestDecoders, ManifestEndpoints,
    ManifestSource, ManifestSourceKind, ModelAvailability, ModelRoute, ModelRouteConfig,
    ParameterDefaultMode, ParameterId, ParameterSpec, ParameterType, ProviderConnection,
    ProviderManifest, ProviderNetworkMode, ProviderParameterMapping, ProviderParameterTarget,
    ProviderTemplate, ProviderTemplateId, TemplateSource, UiParameterLevel,
};
use reqwest::{RequestBuilder, Response, StatusCode, header::ACCEPT};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use url::Url;

use crate::{
    Provider,
    anthropic_messages::AnthropicMessagesProvider,
    discovery::contains_credential_like_token,
    embedding::{
        EmbeddingContract, EmbeddingProvider, EmbeddingRequestSchema, HttpEmbeddingProvider,
        MAX_EMBEDDING_DIMENSIONS,
    },
    gemini_generate_content::{GeminiGenerateContentProvider, GeminiResponseMode},
    manifest_validator::{validate_connection_fields, validate_manifest},
    network_transport::{ProviderHttpTarget, authorize_request, validate_credential_for_auth},
    ollama_native::OllamaNativeProvider,
    openai_compatible::OpenAiCompatibleProvider,
    openai_responses::OpenAiResponsesProvider,
    parameter_mapping::{ParameterEngine, ProviderRequestPlan},
    request_plan::{RequestPreview, build_request_preview, planned_json_payload},
    url_policy::{
        ApprovedLocalNetworkOrigin, IpAddressClass, UrlNetworkBoundary, UrlPolicy,
        classify_ip_address,
    },
};

const MAX_TIMEOUT_SECONDS: u32 = 600;
const HARD_MAX_MODEL_PAGES: usize = 32;
const HARD_MAX_MODELS: usize = 10_000;
const HARD_MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;
const HARD_MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_MODEL_ID_BYTES: usize = 512;
const MAX_GEMINI_MODEL_ID_BYTES: usize = 256;
const MAX_DISPLAY_NAME_BYTES: usize = 1_024;
const MAX_METHOD_COUNT: usize = 64;
const MAX_METHOD_BYTES: usize = 128;
const MAX_SUPPORTED_PARAMETER_COUNT: usize = 64;
const MAX_SUPPORTED_PARAMETER_BYTES: usize = 128;
const MAX_REASONING_EFFORT_COUNT: usize = 64;
const MAX_REASONING_EFFORT_BYTES: usize = 128;
const MAX_CURSOR_BYTES: usize = 2_048;
const ANTHROPIC_VERSION: &str = "2023-06-01";
const BUILT_IN_TEMPLATE_VERSION: u32 = 3;
const MAX_GENERATION_TOKENS: f64 = 4_294_967_295.0;

pub const OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR: &str =
    "opaque reasoning state preservation is not supported by this exact provider template";

/// Stable IDs for provider templates shipped with `LorePia`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltInTemplateId {
    OpenAiResponses,
    OpenAiChatCompatible,
    AnthropicMessages,
    GeminiGenerateContent,
    OpenRouter,
    OllamaNative,
}

impl BuiltInTemplateId {
    pub const ALL: [Self; 6] = [
        Self::OpenAiResponses,
        Self::OpenAiChatCompatible,
        Self::AnthropicMessages,
        Self::GeminiGenerateContent,
        Self::OpenRouter,
        Self::OllamaNative,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "openai-responses-v1",
            Self::OpenAiChatCompatible => "openai-chat-compatible-v1",
            Self::AnthropicMessages => "anthropic-messages-v1",
            Self::GeminiGenerateContent => "gemini-generate-content-v1",
            Self::OpenRouter => "openrouter-v1",
            Self::OllamaNative => "ollama-native-v1",
        }
    }

    pub const fn family(self) -> ApiFamily {
        match self {
            Self::OpenAiResponses => ApiFamily::OpenAiResponses,
            Self::OpenAiChatCompatible | Self::OpenRouter => ApiFamily::OpenAiChatCompletions,
            Self::AnthropicMessages => ApiFamily::AnthropicMessages,
            Self::GeminiGenerateContent => ApiFamily::GeminiGenerateContent,
            Self::OllamaNative => ApiFamily::OllamaNative,
        }
    }

    /// The closed, compiled-in base path for this template.
    ///
    /// This is intentionally keyed by template identity rather than by API
    /// family: `OpenRouter` reuses the `OpenAI` Chat Completions wire family but
    /// serves it below `/api/v1`.
    pub const fn default_api_base_path(self) -> &'static str {
        match self {
            Self::OpenAiResponses | Self::OpenAiChatCompatible | Self::AnthropicMessages => "/v1",
            Self::GeminiGenerateContent => "/v1beta",
            Self::OpenRouter => "/api/v1",
            Self::OllamaNative => "/api",
        }
    }
}

/// The fixed wire contract implemented by one compiled-in adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterDescriptor {
    pub family: ApiFamily,
    pub default_api_base_path: EndpointPath,
    pub default_network_mode: ProviderNetworkMode,
    pub models_endpoint: EndpointPath,
    pub generation_endpoint: EndpointPath,
    /// Closed provider-native embedding route, when this family implements one.
    pub embedding_endpoint: Option<EndpointPath>,
    pub auth: AuthBinding,
    pub response_decoder: DecoderId,
    pub streaming_decoder: DecoderId,
}

/// Provenance category attached to every normalized model record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRecordSource {
    ProviderApi,
}

/// Closed model capabilities that a provider-owned model catalog can verify.
///
/// This deliberately excludes arbitrary provider parameter names. Unknown
/// names remain unknown instead of becoming durable capability claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListedModelCapability {
    Reasoning,
    ToolCalling,
    ParallelToolCalling,
    StructuredOutput,
    JsonMode,
    Logprobs,
    Seed,
}

/// Closed `OpenRouter` request parameters currently documented by the models
/// API and safe to retain as route metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenRouterSupportedParameter {
    FrequencyPenalty,
    IncludeReasoning,
    Logprobs,
    MaxCompletionTokens,
    MaxTokens,
    ParallelToolCalls,
    PresencePenalty,
    Reasoning,
    ReasoningEffort,
    ResponseFormat,
    Seed,
    Stop,
    StructuredOutputs,
    Temperature,
    ToolChoice,
    Tools,
    TopP,
}

impl OpenRouterSupportedParameter {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "frequency_penalty" => Some(Self::FrequencyPenalty),
            "include_reasoning" => Some(Self::IncludeReasoning),
            "logprobs" => Some(Self::Logprobs),
            "max_completion_tokens" => Some(Self::MaxCompletionTokens),
            "max_tokens" => Some(Self::MaxTokens),
            "parallel_tool_calls" => Some(Self::ParallelToolCalls),
            "presence_penalty" => Some(Self::PresencePenalty),
            "reasoning" => Some(Self::Reasoning),
            "reasoning_effort" => Some(Self::ReasoningEffort),
            "response_format" => Some(Self::ResponseFormat),
            "seed" => Some(Self::Seed),
            "stop" => Some(Self::Stop),
            "structured_outputs" => Some(Self::StructuredOutputs),
            "temperature" => Some(Self::Temperature),
            "tool_choice" => Some(Self::ToolChoice),
            "tools" => Some(Self::Tools),
            "top_p" => Some(Self::TopP),
            _ => None,
        }
    }
}

/// Presence and exactness of an `OpenRouter` `supported_parameters` field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "values", rename_all = "snake_case")]
pub enum OpenRouterSupportedParameterSupport {
    #[default]
    NotExposed,
    Exact(Vec<OpenRouterSupportedParameter>),
}

impl OpenRouterSupportedParameterSupport {
    fn is_not_exposed(&self) -> bool {
        matches!(self, Self::NotExposed)
    }
}

/// Exact `OpenRouter` gateway reasoning effort values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenRouterReasoningEffort {
    Max,
    Xhigh,
    High,
    Medium,
    Low,
    Minimal,
    None,
}

impl OpenRouterReasoningEffort {
    pub(crate) const ALL: [Self; 7] = [
        Self::Max,
        Self::Xhigh,
        Self::High,
        Self::Medium,
        Self::Low,
        Self::Minimal,
        Self::None,
    ];

    const fn canonical_index(self) -> usize {
        match self {
            Self::Max => 0,
            Self::Xhigh => 1,
            Self::High => 2,
            Self::Medium => 3,
            Self::Low => 4,
            Self::Minimal => 5,
            Self::None => 6,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "max" => Some(Self::Max),
            "xhigh" => Some(Self::Xhigh),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            "minimal" => Some(Self::Minimal),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// Whether `OpenRouter` exposes an effort selector for one reasoning model.
///
/// The three variants preserve the provider's documented distinction between
/// an omitted field, an explicit `null`, and an exact array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "values", rename_all = "snake_case")]
pub enum OpenRouterReasoningEffortSupport {
    NotExposed,
    AllGateway,
    Exact(Vec<OpenRouterReasoningEffort>),
}

/// Closed, bounded `OpenRouter` reasoning metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListedModelReasoningCapability {
    pub supported_efforts: OpenRouterReasoningEffortSupport,
    pub default_effort: Option<OpenRouterReasoningEffort>,
    pub default_enabled: Option<bool>,
    pub supports_max_tokens: Option<bool>,
    pub mandatory: Option<bool>,
}

/// Typed capability metadata retained from a provider model record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListedModelCapabilities {
    #[serde(default)]
    pub supported: Vec<ListedModelCapability>,
    #[serde(
        default,
        skip_serializing_if = "OpenRouterSupportedParameterSupport::is_not_exposed"
    )]
    pub parameters: OpenRouterSupportedParameterSupport,
    pub reasoning: Option<ListedModelReasoningCapability>,
}

/// A provider model record reduced to bounded, portable fields.
///
/// Provider responses are deliberately not retained wholesale. This prevents a
/// provider from reflecting credentials or unbounded metadata into persistence
/// and keeps route reconciliation independent from provider-specific JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedModel {
    pub model_id: String,
    pub display_name: Option<String>,
    pub max_input_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supported_generation_methods: Vec<String>,
    #[serde(default)]
    pub capabilities: ListedModelCapabilities,
    pub source: ModelRecordSource,
    pub availability: ModelAvailability,
}

/// Exact non-secret source of a successful model listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelListProvenance {
    pub source: ModelRecordSource,
    pub api_family: ApiFamily,
    pub api_origin: CanonicalOrigin,
    pub endpoint_path: EndpointPath,
}

/// A complete, bounded model-list observation suitable for reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelListResult {
    pub models: Vec<ListedModel>,
    pub provenance: ModelListProvenance,
    pub pages_fetched: u32,
    pub response_bytes: u64,
}

/// Whether a manifest exposes a provider-owned model-list endpoint.
///
/// A generation-only provider is a valid provider configuration. Keeping this
/// state typed prevents callers from confusing "no endpoint was declared" with
/// a successful request that happened to return an empty catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelListSupport {
    Supported,
    GenerationOnly,
}

/// Caller-selectable limits which can only tighten `LorePia`'s hard bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelListBudget {
    pages: usize,
    models: usize,
    page_bytes: usize,
    total_bytes: usize,
}

impl ModelListBudget {
    pub fn new(
        max_pages: usize,
        max_models: usize,
        max_page_bytes: usize,
        max_total_bytes: usize,
    ) -> CoreResult<Self> {
        let budget = Self {
            pages: max_pages,
            models: max_models,
            page_bytes: max_page_bytes,
            total_bytes: max_total_bytes,
        };
        budget.validate()?;
        Ok(budget)
    }

    pub const fn max_pages(self) -> usize {
        self.pages
    }

    pub const fn max_models(self) -> usize {
        self.models
    }

    pub const fn max_page_bytes(self) -> usize {
        self.page_bytes
    }

    pub const fn max_total_bytes(self) -> usize {
        self.total_bytes
    }

    fn validate(self) -> CoreResult<()> {
        if self.pages == 0 || self.pages > HARD_MAX_MODEL_PAGES {
            return Err(CoreError::invalid(format!(
                "model-list page budget must be from 1 to {HARD_MAX_MODEL_PAGES}"
            )));
        }
        if self.models == 0 || self.models > HARD_MAX_MODELS {
            return Err(CoreError::invalid(format!(
                "model-list record budget must be from 1 to {HARD_MAX_MODELS}"
            )));
        }
        if self.page_bytes == 0 || self.page_bytes > HARD_MAX_PAGE_BYTES {
            return Err(CoreError::invalid(format!(
                "model-list page byte budget must be from 1 to {HARD_MAX_PAGE_BYTES}"
            )));
        }
        if self.total_bytes == 0
            || self.total_bytes > HARD_MAX_TOTAL_BYTES
            || self.total_bytes < self.page_bytes
        {
            return Err(CoreError::invalid(format!(
                "model-list total byte budget must include one page and not exceed \
                 {HARD_MAX_TOTAL_BYTES}"
            )));
        }
        Ok(())
    }
}

impl Default for ModelListBudget {
    fn default() -> Self {
        Self {
            pages: HARD_MAX_MODEL_PAGES,
            models: HARD_MAX_MODELS,
            page_bytes: HARD_MAX_PAGE_BYTES,
            total_bytes: HARD_MAX_TOTAL_BYTES,
        }
    }
}

/// One model-list operation. The raw credential is borrowed only for the call.
pub struct ModelListRequest<'a> {
    credential: Option<&'a str>,
    cancelled: watch::Receiver<bool>,
    budget: ModelListBudget,
}

impl<'a> ModelListRequest<'a> {
    pub fn new(credential: Option<&'a str>, cancelled: watch::Receiver<bool>) -> Self {
        Self {
            credential,
            cancelled,
            budget: ModelListBudget::default(),
        }
    }

    #[must_use]
    pub fn with_budget(mut self, budget: ModelListBudget) -> Self {
        self.budget = budget;
        self
    }
}

/// Model discovery implemented by a compiled-in, closed adapter family.
#[async_trait]
pub trait ModelListing: Send + Sync {
    fn support(&self) -> ModelListSupport;

    async fn list_models(&self, request: ModelListRequest<'_>) -> CoreResult<ModelListResult>;
}

/// Closed registry for built-in generation and model-list adapters.
#[derive(Debug, Default, Clone, Copy)]
pub struct AdapterRegistry;

impl AdapterRegistry {
    pub const fn new() -> Self {
        Self
    }

    /// Reports whether this release may retain provider-native opaque reasoning.
    ///
    /// Credential-bearing built-ins cannot locally verify account identity, so
    /// the root release invariant disables opaque continuity for every template.
    pub const fn template_supports_opaque_reasoning_state(_template: &ProviderTemplate) -> bool {
        false
    }

    /// Resolves only exact, versioned API-family wire names.
    ///
    /// This intentionally does not deserialize an "other" or generic adapter
    /// value. A newly introduced family must add a compiled adapter and an
    /// explicit match arm.
    pub fn family_from_wire(value: &str) -> CoreResult<ApiFamily> {
        match value {
            "openai_responses" => Ok(ApiFamily::OpenAiResponses),
            "openai_chat_completions" => Ok(ApiFamily::OpenAiChatCompletions),
            "anthropic_messages" => Ok(ApiFamily::AnthropicMessages),
            "gemini_generate_content" => Ok(ApiFamily::GeminiGenerateContent),
            "ollama_native" => Ok(ApiFamily::OllamaNative),
            _ => Err(CoreError::new(
                CoreErrorCode::UnsupportedContent,
                "provider API family is not implemented by this build",
                false,
            )),
        }
    }

    pub fn descriptor(family: ApiFamily) -> CoreResult<AdapterDescriptor> {
        let descriptor = match family {
            ApiFamily::OpenAiResponses => AdapterDescriptor {
                family,
                default_api_base_path: endpoint_path("/v1")?,
                default_network_mode: ProviderNetworkMode::Public,
                models_endpoint: endpoint_path("/models")?,
                generation_endpoint: endpoint_path("/responses")?,
                embedding_endpoint: Some(endpoint_path("/embeddings")?),
                auth: AuthBinding::BearerHeader,
                response_decoder: DecoderId::OpenAiJsonV1,
                streaming_decoder: DecoderId::OpenAiSseV1,
            },
            ApiFamily::OpenAiChatCompletions => AdapterDescriptor {
                family,
                default_api_base_path: endpoint_path("/v1")?,
                default_network_mode: ProviderNetworkMode::Public,
                models_endpoint: endpoint_path("/models")?,
                generation_endpoint: endpoint_path("/chat/completions")?,
                embedding_endpoint: Some(endpoint_path("/embeddings")?),
                auth: AuthBinding::BearerHeader,
                response_decoder: DecoderId::OpenAiJsonV1,
                streaming_decoder: DecoderId::OpenAiSseV1,
            },
            ApiFamily::AnthropicMessages => AdapterDescriptor {
                family,
                default_api_base_path: endpoint_path("/v1")?,
                default_network_mode: ProviderNetworkMode::Public,
                models_endpoint: endpoint_path("/models")?,
                generation_endpoint: endpoint_path("/messages")?,
                embedding_endpoint: None,
                auth: AuthBinding::HeaderApiKey {
                    header_name: domain_header_name("x-api-key")?,
                },
                response_decoder: DecoderId::AnthropicJsonV1,
                streaming_decoder: DecoderId::AnthropicSseV1,
            },
            ApiFamily::GeminiGenerateContent => AdapterDescriptor {
                family,
                default_api_base_path: endpoint_path("/v1beta")?,
                default_network_mode: ProviderNetworkMode::Public,
                models_endpoint: endpoint_path("/models")?,
                // The model ID and `:generateContent` method are expanded by the
                // compiled Gemini adapter below this fixed family root.
                generation_endpoint: endpoint_path("/models")?,
                embedding_endpoint: Some(endpoint_path("/models")?),
                auth: AuthBinding::HeaderApiKey {
                    header_name: domain_header_name("x-goog-api-key")?,
                },
                response_decoder: DecoderId::GeminiJsonV1,
                streaming_decoder: DecoderId::GeminiSseV1,
            },
            ApiFamily::OllamaNative => AdapterDescriptor {
                family,
                default_api_base_path: endpoint_path("/api")?,
                default_network_mode: ProviderNetworkMode::LocalLoopback,
                models_endpoint: endpoint_path("/tags")?,
                generation_endpoint: endpoint_path("/chat")?,
                embedding_endpoint: Some(endpoint_path("/embed")?),
                auth: AuthBinding::None,
                response_decoder: DecoderId::OllamaJsonV1,
                streaming_decoder: DecoderId::OllamaJsonlV1,
            },
        };
        Ok(descriptor)
    }

    pub fn built_in_templates() -> CoreResult<Vec<ProviderTemplate>> {
        BuiltInTemplateId::ALL
            .into_iter()
            .map(Self::built_in_template)
            .collect()
    }

    #[allow(clippy::too_many_lines)]
    pub fn built_in_template(id: BuiltInTemplateId) -> CoreResult<ProviderTemplate> {
        let descriptor = Self::descriptor(id.family())?;
        let (display_name, default_origin, sources, connection_fields) = match id {
            BuiltInTemplateId::OpenAiResponses => (
                "OpenAI",
                Some(canonical_origin("https://api.openai.com")?),
                vec![
                    manifest_source(
                        ManifestSourceKind::OfficialSite,
                        "https://platform.openai.com/docs",
                    )?,
                    manifest_source(
                        ManifestSourceKind::OfficialDocumentation,
                        "https://platform.openai.com/docs/api-reference/models",
                    )?,
                    manifest_source(
                        ManifestSourceKind::OfficialDocumentation,
                        "https://developers.openai.com/api/reference/resources/embeddings/methods/create",
                    )?,
                ],
                vec![api_key_field()],
            ),
            BuiltInTemplateId::OpenAiChatCompatible => (
                "Custom OpenAI-compatible Chat",
                None,
                Vec::new(),
                vec![api_base_url_field(true), api_key_field()],
            ),
            BuiltInTemplateId::AnthropicMessages => (
                "Anthropic",
                Some(canonical_origin("https://api.anthropic.com")?),
                vec![
                    manifest_source(
                        ManifestSourceKind::OfficialSite,
                        "https://platform.claude.com/docs",
                    )?,
                    manifest_source(
                        ManifestSourceKind::OfficialDocumentation,
                        "https://platform.claude.com/docs/en/api/models/list",
                    )?,
                ],
                vec![api_key_field()],
            ),
            BuiltInTemplateId::GeminiGenerateContent => (
                "Google Gemini",
                Some(canonical_origin(
                    "https://generativelanguage.googleapis.com",
                )?),
                vec![
                    manifest_source(
                        ManifestSourceKind::OfficialSite,
                        "https://ai.google.dev/gemini-api/docs",
                    )?,
                    manifest_source(
                        ManifestSourceKind::OfficialDocumentation,
                        "https://ai.google.dev/api/models",
                    )?,
                    manifest_source(
                        ManifestSourceKind::OfficialDocumentation,
                        "https://ai.google.dev/api/embeddings",
                    )?,
                ],
                vec![api_key_field()],
            ),
            BuiltInTemplateId::OpenRouter => (
                "OpenRouter",
                Some(canonical_origin("https://openrouter.ai")?),
                vec![
                    manifest_source(
                        ManifestSourceKind::OfficialSite,
                        "https://openrouter.ai/docs",
                    )?,
                    manifest_source(
                        ManifestSourceKind::OfficialDocumentation,
                        "https://openrouter.ai/docs/api/api-reference/models/get-models",
                    )?,
                    manifest_source(
                        ManifestSourceKind::OfficialDocumentation,
                        "https://openrouter.ai/docs/api/api-reference/chat/send-chat-completion-request",
                    )?,
                    manifest_source(
                        ManifestSourceKind::OfficialDocumentation,
                        "https://openrouter.ai/docs/api/api-reference/embeddings/create-embeddings",
                    )?,
                ],
                vec![api_key_field()],
            ),
            BuiltInTemplateId::OllamaNative => (
                "Ollama",
                Some(canonical_origin("http://localhost:11434")?),
                vec![
                    manifest_source(ManifestSourceKind::OfficialSite, "https://docs.ollama.com")?,
                    manifest_source(
                        ManifestSourceKind::OfficialDocumentation,
                        "https://docs.ollama.com/api/tags",
                    )?,
                    manifest_source(
                        ManifestSourceKind::OfficialDocumentation,
                        "https://docs.ollama.com/api/embed",
                    )?,
                ],
                vec![api_base_url_field(false)],
            ),
        };

        let template = ProviderTemplate {
            id: ProviderTemplateId::from(id.as_str()),
            display_name: display_name.to_owned(),
            manifest_version: BUILT_IN_TEMPLATE_VERSION,
            source: TemplateSource::BuiltIn,
            api_family: descriptor.family,
            connection_fields,
            default_manifest: ProviderManifest {
                schema_version: 1,
                api_family: descriptor.family,
                sources,
                default_api_origin: default_origin,
                auth: descriptor.auth.clone(),
                endpoints: ManifestEndpoints {
                    models: Some(EndpointSpec {
                        method: HttpMethod::Get,
                        path: descriptor.models_endpoint.clone(),
                    }),
                    generate: EndpointSpec {
                        method: HttpMethod::Post,
                        path: descriptor.generation_endpoint.clone(),
                    },
                    embeddings: descriptor
                        .embedding_endpoint
                        .clone()
                        .map(|path| EndpointSpec {
                            method: HttpMethod::Post,
                            path,
                        }),
                },
                decoders: ManifestDecoders {
                    response: descriptor.response_decoder,
                    streaming: Some(descriptor.streaming_decoder),
                },
                parameters: built_in_parameter_specs(descriptor.family),
            },
        };
        validate_template_contract(&template, &descriptor)?;
        Ok(template)
    }

    /// Builds the generation adapter for a validated connection/template pair.
    pub fn build_provider(
        &self,
        template: &ProviderTemplate,
        connection: &ProviderConnection,
    ) -> CoreResult<Arc<dyn Provider>> {
        self.build_provider_with_plan(template, connection, None)
    }

    /// Builds a generation adapter with a preset-specific plan that already
    /// passed the parameter and capability gates.
    pub fn build_provider_with_plan(
        &self,
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        request_plan: Option<ProviderRequestPlan>,
    ) -> CoreResult<Arc<dyn Provider>> {
        let policy = default_network_policy(connection)?;
        Self::build_provider_internal(template, connection, None, request_plan, &policy)
    }

    /// Builds a provider with an explicit typed network boundary.
    ///
    /// `ApprovedLocalNetwork` can only enter through this overload and remains
    /// bound to the connection's exact canonical origin.
    pub fn build_provider_with_network_policy(
        &self,
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        policy: &UrlPolicy,
    ) -> CoreResult<Arc<dyn Provider>> {
        Self::build_provider_internal(template, connection, None, None, policy)
    }

    pub fn build_provider_with_plan_and_network_policy(
        &self,
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        request_plan: Option<ProviderRequestPlan>,
        policy: &UrlPolicy,
    ) -> CoreResult<Arc<dyn Provider>> {
        Self::build_provider_internal(template, connection, None, request_plan, policy)
    }

    /// Builds a provider for one fully resolved model route.
    ///
    /// Current closed families support an exact endpoint-path override. Azure,
    /// Vertex, Bedrock, and other deployment/region/value mappings require
    /// dedicated future adapter families and therefore fail closed here.
    pub fn build_provider_for_route_with_plan(
        &self,
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        route: &ModelRoute,
        request_plan: Option<ProviderRequestPlan>,
    ) -> CoreResult<Arc<dyn Provider>> {
        let policy = default_network_policy(connection)?;
        Self::build_provider_internal(template, connection, Some(route), request_plan, &policy)
    }

    pub fn build_provider_for_route_with_plan_and_network_policy(
        &self,
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        route: &ModelRoute,
        request_plan: Option<ProviderRequestPlan>,
        policy: &UrlPolicy,
    ) -> CoreResult<Arc<dyn Provider>> {
        Self::build_provider_internal(template, connection, Some(route), request_plan, policy)
    }

    /// Builds one exact provider-native embedding adapter.
    ///
    /// The selected route, model, dimensions, manifest hash, endpoint and wire
    /// schema are fixed here before a worker is allowed to borrow a credential.
    /// Route endpoint overrides are deliberately not reused because generation
    /// and embedding endpoints are separate capabilities.
    pub fn build_embedding_provider_for_route(
        &self,
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        route: &ModelRoute,
        dimensions: u32,
    ) -> CoreResult<Arc<dyn EmbeddingProvider>> {
        let policy = default_network_policy(connection)?;
        Self::build_embedding_provider_internal(template, connection, route, dimensions, &policy)
    }

    pub fn build_embedding_provider_for_route_with_network_policy(
        &self,
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        route: &ModelRoute,
        dimensions: u32,
        policy: &UrlPolicy,
    ) -> CoreResult<Arc<dyn EmbeddingProvider>> {
        Self::build_embedding_provider_internal(template, connection, route, dimensions, policy)
    }

    /// Returns the exact destination/header/body-shape contract for one route
    /// without DNS, HTTP, credentials, prompts, or scalar request values.
    pub fn preview_provider_request(
        &self,
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        route: &ModelRoute,
        request_plan: Option<&ProviderRequestPlan>,
    ) -> CoreResult<RequestPreview> {
        let policy = default_network_policy(connection)?;
        Self::preview_provider_request_internal(template, connection, route, request_plan, &policy)
    }

    /// Returns a request preview under an explicit typed network boundary.
    ///
    /// This is required for an exact-origin local-network approval because the
    /// default connection modes intentionally cannot represent broad LAN
    /// access.
    pub fn preview_provider_request_with_network_policy(
        &self,
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        route: &ModelRoute,
        request_plan: Option<&ProviderRequestPlan>,
        policy: &UrlPolicy,
    ) -> CoreResult<RequestPreview> {
        Self::preview_provider_request_internal(template, connection, route, request_plan, policy)
    }

    fn preview_provider_request_internal(
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        route: &ModelRoute,
        request_plan: Option<&ProviderRequestPlan>,
        policy: &UrlPolicy,
    ) -> CoreResult<RequestPreview> {
        let descriptor = Self::descriptor(template.api_family)?;
        validate_template_and_connection(template, connection, &descriptor, policy)?;
        validate_route_contract(route, connection, &descriptor)?;
        validate_generation_model_capability(route, descriptor.family)?;
        validate_route_config(&route.route_config)?;
        if request_plan.is_some_and(|plan| plan.family != descriptor.family) {
            return Err(CoreError::invalid(
                "provider request plan does not match the adapter API family",
            ));
        }
        if request_plan.is_some_and(|plan| plan.preserve_opaque_reasoning_state)
            && !Self::template_supports_opaque_reasoning_state(template)
        {
            return Err(CoreError::invalid(OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR));
        }
        let generation_path = route
            .route_config
            .endpoint_path
            .as_ref()
            .unwrap_or(&template.default_manifest.endpoints.generate.path);
        let mut destination = join_manifest_endpoint(connection, generation_path)?;
        if descriptor.family == ApiFamily::GeminiGenerateContent {
            destination = gemini_generation_endpoint(
                destination,
                &route.model_id,
                template.default_manifest.decoders.streaming.is_some(),
            )?;
        }

        let body = provider_preview_body(
            descriptor.family,
            request_plan,
            is_exact_built_in_openrouter(template),
        );
        let body = planned_json_payload(&body, descriptor.family, request_plan)?;
        let headers = provider_preview_headers(descriptor.family, &template.default_manifest.auth)?;
        build_request_preview(HttpMethod::Post, &destination, &headers, Some(&body)).map_err(
            |error| {
                CoreError::new(
                    CoreErrorCode::InvalidInput,
                    format!("provider request preview is invalid: {error}"),
                    false,
                )
            },
        )
    }

    fn build_provider_internal(
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        route: Option<&ModelRoute>,
        request_plan: Option<ProviderRequestPlan>,
        policy: &UrlPolicy,
    ) -> CoreResult<Arc<dyn Provider>> {
        let descriptor = Self::descriptor(template.api_family)?;
        validate_template_and_connection(template, connection, &descriptor, policy)?;
        if let Some(plan) = &request_plan
            && plan.family != descriptor.family
        {
            return Err(CoreError::invalid(
                "provider request plan does not match the adapter API family",
            ));
        }
        let default_route_config = ModelRouteConfig::default();
        let route_config = route.map_or(&default_route_config, |route| &route.route_config);
        if let Some(route) = route {
            validate_route_contract(route, connection, &descriptor)?;
            validate_generation_model_capability(route, descriptor.family)?;
        }
        validate_route_config(route_config)?;
        let timeout = Duration::from_secs(u64::from(connection.timeout_seconds));
        let generation_path = route_config
            .endpoint_path
            .as_ref()
            .unwrap_or(&template.default_manifest.endpoints.generate.path);
        let generation_target = manifest_http_target(connection, generation_path, policy, timeout)?;
        let auth = template.default_manifest.auth.clone();

        let provider: Arc<dyn Provider> = match descriptor.family {
            ApiFamily::OpenAiResponses => Arc::new(
                OpenAiResponsesProvider::new_with_manifest_target(generation_target, auth)
                    .with_optional_request_plan(request_plan),
            ),
            ApiFamily::OpenAiChatCompletions => Arc::new(
                OpenAiCompatibleProvider::new_with_manifest_target(generation_target, auth)
                    .with_optional_request_plan(request_plan)
                    .with_openrouter_reasoning_details(is_exact_built_in_openrouter(template)),
            ),
            ApiFamily::AnthropicMessages => Arc::new(
                AnthropicMessagesProvider::new_with_manifest_target(generation_target, auth)
                    .with_optional_request_plan(request_plan),
            ),
            ApiFamily::GeminiGenerateContent => {
                let mode = if template.default_manifest.decoders.streaming.is_some() {
                    GeminiResponseMode::Streaming
                } else {
                    GeminiResponseMode::Unary
                };
                Arc::new(
                    GeminiGenerateContentProvider::new_with_manifest_target(
                        generation_target,
                        auth,
                        mode,
                    )
                    .with_optional_request_plan(request_plan),
                )
            }
            ApiFamily::OllamaNative => {
                let models_target = template
                    .default_manifest
                    .endpoints
                    .models
                    .as_ref()
                    .map(|endpoint| {
                        manifest_http_target(connection, &endpoint.path, policy, timeout)
                    })
                    .transpose()?;
                Arc::new(
                    OllamaNativeProvider::new_with_manifest_targets(
                        generation_target,
                        models_target,
                        auth,
                    )
                    .with_optional_request_plan(request_plan),
                )
            }
        };
        Ok(provider)
    }

    fn build_embedding_provider_internal(
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        route: &ModelRoute,
        dimensions: u32,
        policy: &UrlPolicy,
    ) -> CoreResult<Arc<dyn EmbeddingProvider>> {
        let descriptor = Self::descriptor(template.api_family)?;
        validate_template_and_connection(template, connection, &descriptor, policy)?;
        validate_route_contract(route, connection, &descriptor)?;
        validate_route_config(&route.route_config)?;
        if route.route_config.endpoint_path.is_some() {
            return Err(CoreError::new(
                CoreErrorCode::UnsupportedContent,
                "embedding adapters do not accept generation endpoint overrides",
                false,
            ));
        }
        if !(1..=MAX_EMBEDDING_DIMENSIONS).contains(&dimensions) {
            return Err(CoreError::invalid(format!(
                "embedding dimensions must be from 1 to {MAX_EMBEDDING_DIMENSIONS}"
            )));
        }
        if descriptor.family == ApiFamily::AnthropicMessages {
            return Err(CoreError::new(
                CoreErrorCode::UnsupportedContent,
                "Anthropic Messages does not provide an embedding API",
                false,
            ));
        }
        validate_embedding_model_capability(route, descriptor.family)?;
        let embedding_endpoint = template
            .default_manifest
            .endpoints
            .embeddings
            .as_ref()
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::UnsupportedContent,
                    "provider manifest does not declare an embedding endpoint",
                    false,
                )
            })?;
        if embedding_endpoint.method != HttpMethod::Post {
            return Err(CoreError::new(
                CoreErrorCode::UnsupportedContent,
                "provider embedding endpoint is not a supported POST contract",
                false,
            ));
        }

        let timeout = Duration::from_secs(u64::from(connection.timeout_seconds));
        let mut target =
            manifest_http_target(connection, &embedding_endpoint.path, policy, timeout)?;
        if descriptor.family == ApiFamily::GeminiGenerateContent {
            let endpoint = gemini_embedding_endpoint(target.url().clone(), &route.model_id)?;
            target = ProviderHttpTarget::new(endpoint.as_str(), policy, timeout)?;
            if target.origin().as_string() != connection.api_origin.as_str() {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "provider embedding endpoint escaped the approved credential origin",
                    false,
                ));
            }
        }
        let endpoint_path = endpoint_path(target.url().path())?;
        let request_schema = match descriptor.family {
            ApiFamily::OpenAiChatCompletions if is_exact_built_in_openrouter(template) => {
                EmbeddingRequestSchema::OpenRouterEmbeddingsV1
            }
            ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions => {
                EmbeddingRequestSchema::OpenAiEmbeddingsV1
            }
            ApiFamily::GeminiGenerateContent => EmbeddingRequestSchema::GeminiEmbedContentV1,
            ApiFamily::OllamaNative => EmbeddingRequestSchema::OllamaEmbedV1,
            ApiFamily::AnthropicMessages => {
                return Err(CoreError::new(
                    CoreErrorCode::UnsupportedContent,
                    "Anthropic Messages does not provide an embedding API",
                    false,
                ));
            }
        };
        let manifest_sha256 = validate_manifest(&template.default_manifest)?
            .sha256()
            .to_owned();
        let contract = EmbeddingContract::new(
            connection.id.clone(),
            route.id.clone(),
            descriptor.family,
            connection.api_origin.clone(),
            route.model_id.clone(),
            dimensions,
            endpoint_path,
            manifest_sha256,
            request_schema,
        )?;
        Ok(Arc::new(HttpEmbeddingProvider::new(
            contract,
            target,
            template.default_manifest.auth.clone(),
        )))
    }

    /// Builds a bounded model lister for a validated connection/template pair.
    pub fn build_model_listing(
        &self,
        template: &ProviderTemplate,
        connection: &ProviderConnection,
    ) -> CoreResult<Arc<dyn ModelListing>> {
        let policy = default_network_policy(connection)?;
        self.build_model_listing_with_network_policy(template, connection, &policy)
    }

    pub fn build_model_listing_with_network_policy(
        &self,
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        policy: &UrlPolicy,
    ) -> CoreResult<Arc<dyn ModelListing>> {
        let descriptor = Self::descriptor(template.api_family)?;
        validate_template_and_connection(template, connection, &descriptor, policy)?;
        let Some(models_endpoint) = &template.default_manifest.endpoints.models else {
            return Ok(Arc::new(GenerationOnlyModelListing));
        };
        let timeout = Duration::from_secs(u64::from(connection.timeout_seconds));
        let target = manifest_http_target(connection, &models_endpoint.path, policy, timeout)?;
        let endpoint = target.url().clone();
        let endpoint_path = endpoint_path(endpoint.path())?;

        Ok(Arc::new(HttpModelListing {
            family: descriptor.family,
            response_schema: if is_exact_built_in_openrouter(template) {
                ModelListResponseSchema::OpenRouter
            } else {
                ModelListResponseSchema::FamilyDefault
            },
            endpoint,
            target,
            auth: template.default_manifest.auth.clone(),
            provenance: ModelListProvenance {
                source: ModelRecordSource::ProviderApi,
                api_family: descriptor.family,
                api_origin: connection.api_origin.clone(),
                endpoint_path,
            },
        }))
    }
}

struct HttpModelListing {
    family: ApiFamily,
    response_schema: ModelListResponseSchema,
    endpoint: Url,
    target: ProviderHttpTarget,
    auth: AuthBinding,
    provenance: ModelListProvenance,
}

struct GenerationOnlyModelListing;

#[async_trait]
impl ModelListing for GenerationOnlyModelListing {
    fn support(&self) -> ModelListSupport {
        ModelListSupport::GenerationOnly
    }

    async fn list_models(&self, _request: ModelListRequest<'_>) -> CoreResult<ModelListResult> {
        Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            "provider manifest does not declare a model-list endpoint",
            false,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelListResponseSchema {
    FamilyDefault,
    OpenRouter,
}

#[async_trait]
impl ModelListing for HttpModelListing {
    fn support(&self) -> ModelListSupport {
        ModelListSupport::Supported
    }

    async fn list_models(&self, request: ModelListRequest<'_>) -> CoreResult<ModelListResult> {
        request.budget.validate()?;
        ensure_not_cancelled(&request.cancelled)?;

        let ModelListRequest {
            credential,
            mut cancelled,
            budget,
        } = request;
        validate_credential_for_auth(&self.auth, credential)?;
        let mut cursor = None;
        let mut seen_cursors = HashSet::new();
        let mut models = Vec::new();
        let mut seen_model_ids = HashSet::new();
        let mut pages_fetched = 0_usize;
        let mut response_bytes = 0_usize;

        loop {
            if pages_fetched == budget.pages {
                return Err(provider_unavailable(
                    "provider model list exceeded the page budget",
                ));
            }
            let request_url =
                page_url(&self.endpoint, self.family, cursor.as_ref(), budget.models)?;
            let prepared = self.target.prepare().await?;
            ensure_not_cancelled(&cancelled)?;
            let request_builder = self.authorize(prepared.client().get(request_url), credential)?;
            let response = send_with_cancellation(request_builder, &mut cancelled).await?;
            prepared.validate_response_peer(&response)?;
            ensure_success(response.status())?;

            let body = collect_limited_body(
                response,
                budget.page_bytes,
                budget.total_bytes.saturating_sub(response_bytes),
                &mut cancelled,
            )
            .await?;
            response_bytes = response_bytes
                .checked_add(body.len())
                .ok_or_else(|| provider_unavailable("provider model list size overflowed"))?;
            pages_fetched += 1;

            let page = match self.response_schema {
                ModelListResponseSchema::FamilyDefault => parse_model_page(self.family, &body)?,
                ModelListResponseSchema::OpenRouter => parse_openrouter_models(&body)?,
            };
            for model in page.models {
                if !seen_model_ids.insert(model.model_id.clone()) {
                    return Err(provider_unavailable(
                        "provider model list contained a duplicate model ID",
                    ));
                }
                if models.len() == budget.models {
                    return Err(provider_unavailable(
                        "provider model list exceeded the record budget",
                    ));
                }
                models.push(model);
            }

            let Some(next_cursor) = page.next_cursor else {
                break;
            };
            validate_cursor(&next_cursor)?;
            if !seen_cursors.insert(next_cursor.clone()) {
                return Err(provider_unavailable(
                    "provider model list repeated a pagination cursor",
                ));
            }
            cursor = Some(next_cursor);
        }

        models.sort_by(|left, right| left.model_id.cmp(&right.model_id));
        Ok(ModelListResult {
            models,
            provenance: self.provenance.clone(),
            pages_fetched: u32::try_from(pages_fetched)
                .map_err(|_| CoreError::internal("model-list page count overflowed"))?,
            response_bytes: u64::try_from(response_bytes)
                .map_err(|_| CoreError::internal("model-list byte count overflowed"))?,
        })
    }
}

impl HttpModelListing {
    fn authorize(
        &self,
        request: RequestBuilder,
        credential: Option<&str>,
    ) -> CoreResult<RequestBuilder> {
        let request = request.header(ACCEPT, "application/json");
        let request = authorize_request(request, &self.auth, credential)?;

        if self.family == ApiFamily::AnthropicMessages {
            Ok(request.header("anthropic-version", ANTHROPIC_VERSION))
        } else {
            Ok(request)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedModelPage {
    models: Vec<ListedModel>,
    next_cursor: Option<String>,
}

fn parse_model_page(family: ApiFamily, body: &[u8]) -> CoreResult<ParsedModelPage> {
    match family {
        ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions => parse_openai_models(body),
        ApiFamily::AnthropicMessages => parse_anthropic_models(body),
        ApiFamily::GeminiGenerateContent => parse_gemini_models(body),
        ApiFamily::OllamaNative => parse_ollama_models(body),
    }
}

#[derive(Deserialize)]
struct OpenAiModelList {
    data: Vec<OpenAiModel>,
}

#[derive(Deserialize)]
struct OpenAiModel {
    id: String,
}

fn parse_openai_models(body: &[u8]) -> CoreResult<ParsedModelPage> {
    let page: OpenAiModelList = parse_json(body)?;
    ensure_raw_model_count(page.data.len())?;
    let models = page
        .data
        .into_iter()
        .map(|model| normalized_model(model.id, None, None, None, Vec::new()))
        .collect::<CoreResult<Vec<_>>>()?;
    Ok(ParsedModelPage {
        models,
        next_cursor: None,
    })
}

#[derive(Deserialize)]
struct OpenRouterModelList {
    data: Vec<OpenRouterModel>,
}

#[derive(Deserialize)]
struct OpenRouterModel {
    id: String,
    name: Option<String>,
    context_length: Option<u64>,
    top_provider: Option<OpenRouterTopProvider>,
    #[serde(default)]
    supported_parameters: MissingNullOrValue<Vec<String>>,
    #[serde(default)]
    reasoning: MissingOrValue<OpenRouterReasoningMetadata>,
}

#[derive(Deserialize)]
struct OpenRouterTopProvider {
    context_length: Option<u64>,
    max_completion_tokens: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenRouterReasoningMetadata {
    #[serde(default)]
    supported_efforts: MissingNullOrValue<Vec<String>>,
    #[serde(default)]
    default_effort: MissingOrValue<String>,
    #[serde(default)]
    default_enabled: MissingOrValue<bool>,
    #[serde(default)]
    supports_max_tokens: MissingOrValue<bool>,
    #[serde(default)]
    mandatory: MissingOrValue<bool>,
}

#[derive(Debug, Default)]
enum MissingNullOrValue<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T> Deserialize<'de> for MissingNullOrValue<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(|value| value.map_or(Self::Null, Self::Value))
    }
}

#[derive(Debug, Default)]
enum MissingOrValue<T> {
    #[default]
    Missing,
    Value(T),
}

impl<T> MissingOrValue<T> {
    fn into_option(self) -> Option<T> {
        match self {
            Self::Missing => None,
            Self::Value(value) => Some(value),
        }
    }
}

impl<'de, T> Deserialize<'de> for MissingOrValue<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::Value)
    }
}

fn parse_openrouter_models(body: &[u8]) -> CoreResult<ParsedModelPage> {
    let page: OpenRouterModelList = parse_json(body)?;
    ensure_raw_model_count(page.data.len())?;
    let models = page
        .data
        .into_iter()
        .map(|model| {
            let capabilities = normalize_openrouter_capabilities(
                model.supported_parameters,
                model.reasoning.into_option(),
            )?;
            let top_provider_context = model
                .top_provider
                .as_ref()
                .and_then(|provider| provider.context_length);
            let mut normalized = normalized_model(
                model.id,
                model.name,
                top_provider_context.or(model.context_length),
                model
                    .top_provider
                    .and_then(|provider| provider.max_completion_tokens),
                Vec::new(),
            )?;
            normalized.capabilities = capabilities;
            Ok(normalized)
        })
        .collect::<CoreResult<Vec<_>>>()?;
    Ok(ParsedModelPage {
        models,
        next_cursor: None,
    })
}

fn normalize_openrouter_capabilities(
    supported_parameters: MissingNullOrValue<Vec<String>>,
    reasoning: Option<OpenRouterReasoningMetadata>,
) -> CoreResult<ListedModelCapabilities> {
    let parameters = match supported_parameters {
        MissingNullOrValue::Missing | MissingNullOrValue::Null => {
            return Err(provider_unavailable(
                "provider returned malformed supported parameter metadata",
            ));
        }
        MissingNullOrValue::Value(values) => {
            validate_supported_parameters(&values)?;
            let mut normalized = values
                .iter()
                .filter_map(|value| OpenRouterSupportedParameter::parse(value))
                .collect::<Vec<_>>();
            normalized.sort();
            normalized.dedup();
            OpenRouterSupportedParameterSupport::Exact(normalized)
        }
    };
    let exact_parameters = match &parameters {
        OpenRouterSupportedParameterSupport::Exact(values) => Some(values.as_slice()),
        OpenRouterSupportedParameterSupport::NotExposed => None,
    };
    let mut supported = Vec::new();
    let has_parameter = |candidate: OpenRouterSupportedParameter| {
        exact_parameters.is_some_and(|parameters| parameters.contains(&candidate))
    };
    let has_unified_reasoning = has_parameter(OpenRouterSupportedParameter::Reasoning);
    let has_legacy_reasoning_effort = has_parameter(OpenRouterSupportedParameter::ReasoningEffort);
    if reasoning.is_some() && !has_unified_reasoning && !has_legacy_reasoning_effort {
        return Err(provider_unavailable(
            "provider returned reasoning metadata without a supported reasoning request parameter",
        ));
    }
    let reasoning_supported = has_unified_reasoning || has_legacy_reasoning_effort;
    if reasoning_supported {
        supported.push(ListedModelCapability::Reasoning);
    }
    if has_parameter(OpenRouterSupportedParameter::Tools) {
        supported.push(ListedModelCapability::ToolCalling);
    }
    if has_parameter(OpenRouterSupportedParameter::ParallelToolCalls) {
        if !has_parameter(OpenRouterSupportedParameter::Tools) {
            return Err(provider_unavailable(
                "provider returned parallel tool metadata without tool support",
            ));
        }
        supported.push(ListedModelCapability::ParallelToolCalling);
    }
    if has_parameter(OpenRouterSupportedParameter::StructuredOutputs) {
        supported.push(ListedModelCapability::StructuredOutput);
    }
    if has_parameter(OpenRouterSupportedParameter::ResponseFormat) {
        supported.push(ListedModelCapability::JsonMode);
    }
    if has_parameter(OpenRouterSupportedParameter::Logprobs) {
        supported.push(ListedModelCapability::Logprobs);
    }
    if has_parameter(OpenRouterSupportedParameter::Seed) {
        supported.push(ListedModelCapability::Seed);
    }
    supported.sort();
    supported.dedup();

    let reasoning = reasoning.map(normalize_openrouter_reasoning).transpose()?;
    Ok(ListedModelCapabilities {
        supported,
        parameters,
        reasoning,
    })
}

fn normalize_openrouter_reasoning(
    raw: OpenRouterReasoningMetadata,
) -> CoreResult<ListedModelReasoningCapability> {
    let OpenRouterReasoningMetadata {
        supported_efforts,
        default_effort,
        default_enabled,
        supports_max_tokens,
        mandatory,
    } = raw;
    let default_effort = default_effort
        .into_option()
        .map(|value| {
            validate_reasoning_effort_string(&value)?;
            Ok(OpenRouterReasoningEffort::parse(&value))
        })
        .transpose()?
        .flatten();
    let default_enabled = default_enabled.into_option();
    let supports_max_tokens = supports_max_tokens.into_option();
    let mandatory = mandatory.into_option();
    let supported_efforts = match supported_efforts {
        MissingNullOrValue::Missing => OpenRouterReasoningEffortSupport::NotExposed,
        MissingNullOrValue::Null => OpenRouterReasoningEffortSupport::AllGateway,
        MissingNullOrValue::Value(efforts) => {
            if efforts.len() > MAX_REASONING_EFFORT_COUNT {
                return Err(provider_unavailable(
                    "provider returned invalid reasoning effort metadata",
                ));
            }
            let mut normalized = efforts
                .iter()
                .map(|value| {
                    validate_reasoning_effort_string(value)?;
                    Ok(OpenRouterReasoningEffort::parse(value))
                })
                .collect::<CoreResult<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            normalized.sort_by_key(|effort| effort.canonical_index());
            normalized.dedup();
            OpenRouterReasoningEffortSupport::Exact(normalized)
        }
    };
    if let Some(default_effort) = default_effort {
        let default_is_supported = match &supported_efforts {
            OpenRouterReasoningEffortSupport::NotExposed => false,
            OpenRouterReasoningEffortSupport::AllGateway => {
                OpenRouterReasoningEffort::ALL.contains(&default_effort)
            }
            OpenRouterReasoningEffortSupport::Exact(efforts) => efforts.contains(&default_effort),
        };
        if !default_is_supported {
            return Err(provider_unavailable(
                "provider returned contradictory reasoning defaults",
            ));
        }
    }
    let is_mandatory = mandatory == Some(true);
    let supports_none = match &supported_efforts {
        OpenRouterReasoningEffortSupport::NotExposed => false,
        OpenRouterReasoningEffortSupport::AllGateway => true,
        OpenRouterReasoningEffortSupport::Exact(efforts) => {
            efforts.contains(&OpenRouterReasoningEffort::None)
        }
    };
    if is_mandatory
        && (supports_none
            || default_effort == Some(OpenRouterReasoningEffort::None)
            || default_enabled == Some(false))
        || default_effort == Some(OpenRouterReasoningEffort::None) && default_enabled == Some(true)
    {
        return Err(provider_unavailable(
            "provider returned contradictory reasoning metadata",
        ));
    }
    Ok(ListedModelReasoningCapability {
        supported_efforts,
        default_effort,
        default_enabled,
        supports_max_tokens,
        mandatory,
    })
}

fn validate_reasoning_effort_string(value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > MAX_REASONING_EFFORT_BYTES
        || value.chars().any(char::is_control)
        || contains_credential_like_token(value)
    {
        return Err(provider_unavailable(
            "provider returned invalid reasoning effort metadata",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct AnthropicModelList {
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicModel {
    id: String,
    display_name: Option<String>,
    max_input_tokens: Option<u64>,
    max_tokens: Option<u64>,
}

fn parse_anthropic_models(body: &[u8]) -> CoreResult<ParsedModelPage> {
    let page: AnthropicModelList = parse_json(body)?;
    ensure_raw_model_count(page.data.len())?;
    let models = page
        .data
        .into_iter()
        .map(|model| {
            normalized_model(
                model.id,
                model.display_name,
                model.max_input_tokens,
                model.max_tokens,
                vec!["messages".to_owned()],
            )
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let next_cursor = if page.has_more {
        Some(
            page.last_id
                .filter(|cursor| !cursor.is_empty())
                .ok_or_else(|| {
                    provider_unavailable("provider model list omitted the next pagination cursor")
                })?,
        )
    } else {
        None
    };
    Ok(ParsedModelPage {
        models,
        next_cursor,
    })
}

#[derive(Deserialize)]
struct GeminiModelList {
    models: Vec<GeminiModel>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct GeminiModel {
    name: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "inputTokenLimit")]
    input_token_limit: Option<u64>,
    #[serde(rename = "outputTokenLimit")]
    output_token_limit: Option<u64>,
    #[serde(rename = "supportedGenerationMethods", default)]
    supported_generation_methods: Vec<String>,
}

fn parse_gemini_models(body: &[u8]) -> CoreResult<ParsedModelPage> {
    let page: GeminiModelList = parse_json(body)?;
    ensure_raw_model_count(page.models.len())?;
    let models = page
        .models
        .into_iter()
        .filter(|model| {
            model.supported_generation_methods.iter().any(|method| {
                matches!(
                    method.as_str(),
                    "generateContent" | "streamGenerateContent" | "embedContent"
                )
            })
        })
        .map(|model| {
            let model_id = model
                .name
                .strip_prefix("models/")
                .unwrap_or(&model.name)
                .to_owned();
            validate_gemini_model_id(&model_id)?;
            normalized_model(
                model_id,
                model.display_name,
                model.input_token_limit,
                model.output_token_limit,
                model.supported_generation_methods,
            )
        })
        .collect::<CoreResult<Vec<_>>>()?;
    Ok(ParsedModelPage {
        models,
        next_cursor: page.next_page_token.filter(|cursor| !cursor.is_empty()),
    })
}

#[derive(Deserialize)]
struct OllamaModelList {
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    model: Option<String>,
    name: Option<String>,
}

fn parse_ollama_models(body: &[u8]) -> CoreResult<ParsedModelPage> {
    let page: OllamaModelList = parse_json(body)?;
    ensure_raw_model_count(page.models.len())?;
    let models = page
        .models
        .into_iter()
        .map(|model| {
            let model_id = model
                .model
                .filter(|value| !value.is_empty())
                .or_else(|| model.name.clone().filter(|value| !value.is_empty()))
                .ok_or_else(|| provider_unavailable("provider returned a model without an ID"))?;
            normalized_model(model_id, model.name, None, None, vec!["chat".to_owned()])
        })
        .collect::<CoreResult<Vec<_>>>()?;
    Ok(ParsedModelPage {
        models,
        next_cursor: None,
    })
}

fn normalized_model(
    model_id: String,
    display_name: Option<String>,
    max_input_tokens: Option<u64>,
    max_output_tokens: Option<u64>,
    mut supported_generation_methods: Vec<String>,
) -> CoreResult<ListedModel> {
    validate_model_id(&model_id)?;
    if let Some(display_name) = &display_name {
        validate_display_name(display_name)?;
    }
    validate_methods(&supported_generation_methods)?;
    supported_generation_methods.sort();
    supported_generation_methods.dedup();
    Ok(ListedModel {
        model_id,
        display_name,
        max_input_tokens,
        max_output_tokens,
        supported_generation_methods,
        capabilities: ListedModelCapabilities::default(),
        source: ModelRecordSource::ProviderApi,
        availability: ModelAvailability::Available,
    })
}

fn ensure_raw_model_count(count: usize) -> CoreResult<()> {
    if count > HARD_MAX_MODELS {
        return Err(provider_unavailable(
            "provider model list exceeded the hard record limit",
        ));
    }
    Ok(())
}

fn validate_model_id(value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > MAX_MODEL_ID_BYTES
        || value.trim() != value
        || value.chars().any(char::is_control)
        || contains_credential_like_token(value)
    {
        return Err(provider_unavailable(
            "provider returned an invalid model identifier",
        ));
    }
    Ok(())
}

fn validate_gemini_model_id(value: &str) -> CoreResult<()> {
    validate_model_id(value)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || matches!(value, "." | "..")
    {
        return Err(provider_unavailable(
            "provider returned an invalid Gemini model identifier",
        ));
    }
    Ok(())
}

fn validate_display_name(value: &str) -> CoreResult<()> {
    if value.len() > MAX_DISPLAY_NAME_BYTES
        || value.chars().any(char::is_control)
        || contains_credential_like_token(value)
    {
        return Err(provider_unavailable(
            "provider returned an invalid model display name",
        ));
    }
    Ok(())
}

fn validate_methods(values: &[String]) -> CoreResult<()> {
    if values.len() > MAX_METHOD_COUNT
        || values.iter().any(|value| {
            value.is_empty()
                || value.len() > MAX_METHOD_BYTES
                || value.chars().any(char::is_control)
                || contains_credential_like_token(value)
        })
    {
        return Err(provider_unavailable(
            "provider returned invalid model generation methods",
        ));
    }
    Ok(())
}

fn validate_supported_parameters(values: &[String]) -> CoreResult<()> {
    if values.len() > MAX_SUPPORTED_PARAMETER_COUNT
        || values.iter().any(|value| {
            value.is_empty()
                || value.len() > MAX_SUPPORTED_PARAMETER_BYTES
                || value.chars().any(char::is_control)
                || contains_credential_like_token(value)
        })
    {
        return Err(provider_unavailable(
            "provider returned invalid supported parameter metadata",
        ));
    }
    Ok(())
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &[u8]) -> CoreResult<T> {
    serde_json::from_slice(body)
        .map_err(|_| provider_unavailable("provider returned a malformed model list"))
}

fn page_url(
    endpoint: &Url,
    family: ApiFamily,
    cursor: Option<&String>,
    max_models: usize,
) -> CoreResult<Url> {
    let mut url = endpoint.clone();
    let page_size = max_models.min(1_000).to_string();
    match family {
        ApiFamily::AnthropicMessages => {
            let mut query = url.query_pairs_mut();
            query.append_pair("limit", &page_size);
            if let Some(cursor) = cursor {
                query.append_pair("after_id", cursor);
            }
        }
        ApiFamily::GeminiGenerateContent => {
            let mut query = url.query_pairs_mut();
            query.append_pair("pageSize", &page_size);
            if let Some(cursor) = cursor {
                query.append_pair("pageToken", cursor);
            }
        }
        ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions | ApiFamily::OllamaNative => {
            if cursor.is_some() {
                return Err(CoreError::internal(
                    "non-paginated adapter received a pagination cursor",
                ));
            }
        }
    }
    Ok(url)
}

fn validate_cursor(cursor: &str) -> CoreResult<()> {
    if cursor.is_empty() || cursor.len() > MAX_CURSOR_BYTES || cursor.chars().any(char::is_control)
    {
        return Err(provider_unavailable(
            "provider returned an invalid pagination cursor",
        ));
    }
    Ok(())
}

async fn send_with_cancellation(
    request: RequestBuilder,
    cancelled: &mut watch::Receiver<bool>,
) -> CoreResult<Response> {
    ensure_not_cancelled(cancelled)?;
    let mut response = Box::pin(request.send());
    let mut cancellation_open = true;
    loop {
        tokio::select! {
            biased;
            change = cancelled.changed(), if cancellation_open => {
                if change.is_err() {
                    cancellation_open = false;
                } else {
                    ensure_not_cancelled(cancelled)?;
                }
            }
            result = &mut response => {
                return result.map_err(network_error);
            }
        }
    }
}

async fn collect_limited_body(
    response: Response,
    page_limit: usize,
    total_remaining: usize,
    cancelled: &mut watch::Receiver<bool>,
) -> CoreResult<Vec<u8>> {
    if total_remaining == 0 {
        return Err(provider_unavailable(
            "provider model list exceeded the total byte budget",
        ));
    }
    let effective_limit = page_limit.min(total_remaining);
    if response
        .content_length()
        .is_some_and(|length| length > effective_limit as u64)
    {
        return Err(provider_unavailable(
            "provider model-list response exceeded the byte budget",
        ));
    }

    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    let mut cancellation_open = true;
    loop {
        tokio::select! {
            biased;
            change = cancelled.changed(), if cancellation_open => {
                if change.is_err() {
                    cancellation_open = false;
                } else {
                    ensure_not_cancelled(cancelled)?;
                }
            }
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };
                let chunk = chunk.map_err(network_error)?;
                let next_len = body
                    .len()
                    .checked_add(chunk.len())
                    .ok_or_else(|| provider_unavailable(
                        "provider model-list response size overflowed"
                    ))?;
                if next_len > effective_limit {
                    return Err(provider_unavailable(
                        "provider model-list response exceeded the byte budget",
                    ));
                }
                body.extend_from_slice(&chunk);
            }
        }
    }
    Ok(body)
}

fn ensure_not_cancelled(cancelled: &watch::Receiver<bool>) -> CoreResult<()> {
    if *cancelled.borrow() {
        Err(CoreError::new(
            CoreErrorCode::Cancelled,
            "model listing was cancelled",
            true,
        ))
    } else {
        Ok(())
    }
}

fn ensure_success(status: StatusCode) -> CoreResult<()> {
    if status.is_success() {
        return Ok(());
    }
    let code = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => CoreErrorCode::ProviderAuthFailed,
        StatusCode::TOO_MANY_REQUESTS => CoreErrorCode::ProviderRateLimited,
        _ => CoreErrorCode::ProviderUnavailable,
    };
    Err(CoreError::new(
        code,
        format!("provider model listing returned HTTP {}", status.as_u16()),
        status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS,
    ))
}

fn network_error(error: reqwest::Error) -> CoreError {
    CoreError::new(
        if error.is_timeout() {
            CoreErrorCode::ProviderUnavailable
        } else {
            CoreErrorCode::NetworkUnavailable
        },
        "provider model-list request failed",
        true,
    )
}

fn provider_unavailable(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::ProviderUnavailable, message, true)
}

fn validate_template_and_connection(
    template: &ProviderTemplate,
    connection: &ProviderConnection,
    descriptor: &AdapterDescriptor,
    policy: &UrlPolicy,
) -> CoreResult<()> {
    validate_template_contract(template, descriptor)?;
    validate_connection_target_material(connection)?;
    if connection.template_id != template.id
        || connection.template_version != template.manifest_version
    {
        return Err(CoreError::invalid(
            "provider connection does not match the selected template version",
        ));
    }
    if !(1..=MAX_TIMEOUT_SECONDS).contains(&connection.timeout_seconds) {
        return Err(CoreError::invalid(format!(
            "provider timeout must be from 1 to {MAX_TIMEOUT_SECONDS} seconds"
        )));
    }
    validate_network_policy(connection, policy)?;
    validate_credential_scope(connection, &template.default_manifest.auth)
}

fn validate_connection_target_material(connection: &ProviderConnection) -> CoreResult<()> {
    let origin = Url::parse(connection.api_origin.as_str())
        .map_err(|_| CoreError::invalid("provider API origin is invalid"))?;
    if origin.host_str().is_some_and(|host| {
        host.parse::<std::net::IpAddr>().is_err()
            && host.split('.').any(contains_credential_like_token)
    }) || connection
        .config
        .api_base_path
        .as_ref()
        .is_some_and(|path| path_contains_credential_like_material(path.as_str()))
    {
        return Err(CoreError::invalid(
            "provider connection target contains credential-like material",
        ));
    }
    Ok(())
}

fn path_contains_credential_like_material(path: &str) -> bool {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .any(|segment| {
            let mut decoded = segment.to_owned();
            for _ in 0..=2 {
                if contains_credential_like_token(&decoded) {
                    return true;
                }
                match percent_decode_once(&decoded) {
                    Ok(Some(next)) => decoded = next,
                    Ok(None) => return false,
                    Err(()) => return true,
                }
            }
            contains_credential_like_token(&decoded)
                || percent_decode_once(&decoded).is_ok_and(|next| next.is_some())
        })
}

fn percent_decode_once(value: &str) -> Result<Option<String>, ()> {
    let bytes = value.as_bytes();
    if !bytes.contains(&b'%') {
        return Ok(None);
    }
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0_usize;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(());
        }
        let high = hex_nibble(bytes[index + 1]).ok_or(())?;
        let low = hex_nibble(bytes[index + 2]).ok_or(())?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).map(Some).map_err(|_| ())
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn validate_template_contract(
    template: &ProviderTemplate,
    descriptor: &AdapterDescriptor,
) -> CoreResult<()> {
    validate_connection_fields(&template.connection_fields)?;
    let validated = validate_manifest(&template.default_manifest)?;
    ParameterEngine::from_manifest_specs_for_family(
        template.api_family,
        &template.default_manifest.parameters,
    )
    .map_err(|error| {
        CoreError::new(
            CoreErrorCode::UnsupportedContent,
            format!("provider parameter contract is invalid: {error}"),
            false,
        )
    })?;
    let manifest = validated.manifest();
    let streaming_supported = manifest.decoders.streaming == Some(descriptor.streaming_decoder)
        || (descriptor.family == ApiFamily::GeminiGenerateContent
            && manifest.decoders.streaming.is_none());
    if template.api_family != descriptor.family
        || manifest.api_family != descriptor.family
        || manifest.decoders.response != descriptor.response_decoder
        || !streaming_supported
    {
        return Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            "provider template requests an adapter contract not implemented by this build",
            false,
        ));
    }
    Ok(())
}

fn validate_route_contract(
    route: &ModelRoute,
    connection: &ProviderConnection,
    descriptor: &AdapterDescriptor,
) -> CoreResult<()> {
    if route.connection_id != connection.id || route.api_family != descriptor.family {
        return Err(CoreError::invalid(
            "model route does not match the provider connection and API family",
        ));
    }
    if route.model_id.trim().is_empty()
        || route.model_id.trim() != route.model_id
        || route.model_id.len() > MAX_MODEL_ID_BYTES
        || route.model_id.chars().any(char::is_control)
        || contains_credential_like_token(&route.model_id)
    {
        return Err(CoreError::invalid(
            "model route has an invalid model identifier",
        ));
    }
    Ok(())
}

fn validate_generation_model_capability(route: &ModelRoute, family: ApiFamily) -> CoreResult<()> {
    if family != ApiFamily::GeminiGenerateContent {
        return Ok(());
    }
    validate_gemini_listed_capability(
        route,
        |method| matches!(method, "generateContent" | "streamGenerateContent"),
        "selected Gemini model does not advertise generateContent",
    )
}

fn validate_embedding_model_capability(route: &ModelRoute, family: ApiFamily) -> CoreResult<()> {
    if family != ApiFamily::GeminiGenerateContent {
        return Ok(());
    }
    validate_gemini_listed_capability(
        route,
        |method| method == "embedContent",
        "selected Gemini model does not advertise embedContent",
    )
}

fn validate_gemini_listed_capability(
    route: &ModelRoute,
    supported: impl Fn(&str) -> bool,
    unsupported_message: &'static str,
) -> CoreResult<()> {
    let Some(raw_metadata) = &route.raw_metadata else {
        return Ok(());
    };
    let value: serde_json::Value = serde_json::from_str(raw_metadata.as_str())
        .map_err(|_| CoreError::invalid("model route has invalid provider metadata"))?;
    let Some(methods) = value.get("supported_generation_methods") else {
        return Ok(());
    };
    let methods = methods
        .as_array()
        .ok_or_else(|| CoreError::invalid("model route has invalid provider method metadata"))?;
    if methods
        .iter()
        .filter_map(serde_json::Value::as_str)
        .any(supported)
    {
        Ok(())
    } else {
        Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            unsupported_message,
            false,
        ))
    }
}

fn validate_route_config(config: &ModelRouteConfig) -> CoreResult<()> {
    if config
        .endpoint_path
        .as_ref()
        .is_some_and(|path| path_contains_credential_like_material(path.as_str()))
    {
        return Err(CoreError::invalid(
            "model route endpoint contains credential-like material",
        ));
    }
    if config.deployment_id.is_some() || config.region.is_some() || !config.values.is_empty() {
        return Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            "this adapter family cannot apply deployment, region, or route-specific values",
            false,
        ));
    }
    Ok(())
}

fn default_network_policy(connection: &ProviderConnection) -> CoreResult<UrlPolicy> {
    match (
        connection.config.network_mode,
        connection.config.local_network_approval.as_ref(),
    ) {
        (ProviderNetworkMode::Public, None) => Ok(UrlPolicy::public()),
        (ProviderNetworkMode::LocalLoopback, None) => Ok(UrlPolicy::local_loopback()),
        (ProviderNetworkMode::ApprovedLocalNetwork, Some(approval)) => {
            let approval =
                ApprovedLocalNetworkOrigin::new(approval.origin.as_str(), &approval.addresses)
                    .map_err(|_| {
                        CoreError::invalid(
                            "provider local-network approval is invalid for this connection",
                        )
                    })?;
            Ok(UrlPolicy::approved_local_network(approval))
        }
        (ProviderNetworkMode::ApprovedLocalNetwork, None) => Err(CoreError::invalid(
            "approved local-network mode requires an exact origin and address approval",
        )),
        (ProviderNetworkMode::Public | ProviderNetworkMode::LocalLoopback, Some(_)) => {
            Err(CoreError::invalid(
                "local-network approval is only valid in approved local-network mode",
            ))
        }
    }
}

fn validate_network_policy(connection: &ProviderConnection, policy: &UrlPolicy) -> CoreResult<()> {
    match policy.network_boundary() {
        UrlNetworkBoundary::Public
            if connection.config.network_mode != ProviderNetworkMode::Public =>
        {
            return Err(CoreError::invalid(
                "public network policy does not match the provider connection",
            ));
        }
        UrlNetworkBoundary::LocalLoopback
            if connection.config.network_mode != ProviderNetworkMode::LocalLoopback =>
        {
            return Err(CoreError::invalid(
                "loopback network policy does not match the provider connection",
            ));
        }
        UrlNetworkBoundary::ApprovedLocalNetwork
            if connection.config.network_mode != ProviderNetworkMode::ApprovedLocalNetwork =>
        {
            return Err(CoreError::invalid(
                "approved local-network policy does not match the provider connection",
            ));
        }
        UrlNetworkBoundary::Public
        | UrlNetworkBoundary::LocalLoopback
        | UrlNetworkBoundary::ApprovedLocalNetwork => {}
    }

    let origin_url = format!("{}/", connection.api_origin.as_str());
    let canonical = policy.canonicalize(&origin_url).map_err(|_| {
        let private_literal = Url::parse(&origin_url)
            .ok()
            .and_then(|url| url.host_str()?.parse().ok())
            .is_some_and(|address| classify_ip_address(address) == IpAddressClass::Private);
        if policy.network_boundary() == UrlNetworkBoundary::ApprovedLocalNetwork || private_literal
        {
            CoreError::new(
                CoreErrorCode::PermissionDenied,
                "provider origin is not allowed by the selected network policy",
                false,
            )
        } else {
            CoreError::invalid(
                "provider API origin is not allowed by the selected public or loopback mode",
            )
        }
    })?;
    if canonical.origin().as_string() != connection.api_origin.as_str() {
        return Err(CoreError::new(
            CoreErrorCode::PermissionDenied,
            "network policy approval does not match the provider origin",
            false,
        ));
    }
    Ok(())
}

fn manifest_http_target(
    connection: &ProviderConnection,
    endpoint: &EndpointPath,
    policy: &UrlPolicy,
    timeout: Duration,
) -> CoreResult<ProviderHttpTarget> {
    let url = join_manifest_endpoint(connection, endpoint)?;
    let target = ProviderHttpTarget::new(url.as_str(), policy, timeout)?;
    if target.origin().as_string() != connection.api_origin.as_str() {
        return Err(CoreError::new(
            CoreErrorCode::PermissionDenied,
            "provider endpoint escaped the approved credential origin",
            false,
        ));
    }
    Ok(target)
}

fn join_manifest_endpoint(
    connection: &ProviderConnection,
    endpoint: &EndpointPath,
) -> CoreResult<Url> {
    let mut root = Url::parse(connection.api_origin.as_str())
        .map_err(|_| CoreError::invalid("provider API origin is invalid"))?;
    if let Some(prefix) = &connection.config.api_base_path {
        root.set_path(prefix.as_str());
    }
    join_api_endpoint(&root, endpoint)
}

fn gemini_generation_endpoint(
    mut collection: Url,
    model_id: &str,
    streaming: bool,
) -> CoreResult<Url> {
    let model_id = model_id.strip_prefix("models/").unwrap_or(model_id);
    if model_id.is_empty()
        || model_id.len() > MAX_GEMINI_MODEL_ID_BYTES
        || contains_credential_like_token(model_id)
        || !model_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || matches!(model_id, "." | "..")
    {
        return Err(CoreError::invalid(
            "model route has an invalid Gemini model ID",
        ));
    }
    let method = if streaming {
        format!("{model_id}:streamGenerateContent")
    } else {
        format!("{model_id}:generateContent")
    };
    collection
        .path_segments_mut()
        .map_err(|()| CoreError::invalid("Gemini generation endpoint cannot be extended"))?
        .pop_if_empty()
        .push(&method);
    if streaming {
        collection.query_pairs_mut().append_pair("alt", "sse");
    }
    Ok(collection)
}

fn gemini_embedding_endpoint(mut collection: Url, model_id: &str) -> CoreResult<Url> {
    let model_id = model_id.strip_prefix("models/").unwrap_or(model_id);
    if model_id.is_empty()
        || model_id.len() > MAX_GEMINI_MODEL_ID_BYTES
        || contains_credential_like_token(model_id)
        || !model_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || matches!(model_id, "." | "..")
    {
        return Err(CoreError::invalid(
            "model route has an invalid Gemini embedding model ID",
        ));
    }
    let method = format!("{model_id}:embedContent");
    collection
        .path_segments_mut()
        .map_err(|()| CoreError::invalid("Gemini embedding endpoint cannot be extended"))?
        .pop_if_empty()
        .push(&method);
    Ok(collection)
}

fn provider_preview_headers(
    family: ApiFamily,
    auth: &AuthBinding,
) -> CoreResult<Vec<DomainHeaderName>> {
    let mut headers = vec![domain_header_name("content-type")?];
    match auth {
        AuthBinding::None => {}
        AuthBinding::BearerHeader => headers.push(domain_header_name("authorization")?),
        AuthBinding::HeaderApiKey { header_name } => headers.push(header_name.clone()),
    }
    match family {
        ApiFamily::OpenAiResponses | ApiFamily::GeminiGenerateContent => {
            headers.push(domain_header_name("accept")?);
        }
        ApiFamily::AnthropicMessages => {
            headers.push(domain_header_name("accept")?);
            headers.push(domain_header_name("anthropic-version")?);
        }
        ApiFamily::OpenAiChatCompletions | ApiFamily::OllamaNative => {}
    }
    Ok(headers)
}

fn provider_preview_body(
    family: ApiFamily,
    request_plan: Option<&ProviderRequestPlan>,
    is_built_in_openrouter: bool,
) -> serde_json::Value {
    match family {
        ApiFamily::OpenAiResponses => {
            serde_json::json!({
                "model": null,
                "input": [],
                "stream": true,
                "store": false,
            })
        }
        ApiFamily::OpenAiChatCompletions => {
            let mut body = serde_json::json!({
                "model": null,
                "messages": [],
                "stream": true,
            });
            if is_built_in_openrouter {
                body.as_object_mut()
                    .expect("static preview body is an object")
                    .insert(
                        "stream_options".to_owned(),
                        serde_json::json!({"include_usage": true}),
                    );
            }
            body
        }
        ApiFamily::AnthropicMessages => serde_json::json!({
            "model": null,
            "system": "",
            "messages": [],
            "stream": true,
        }),
        ApiFamily::GeminiGenerateContent => {
            let mut body = serde_json::json!({
                "systemInstruction": {
                    "parts": [{"text": ""}],
                },
                "contents": [{
                    "role": null,
                    "parts": [{"text": ""}],
                }],
            });
            if request_plan.is_none() {
                body.as_object_mut()
                    .expect("static preview body is an object")
                    .insert(
                        "generationConfig".to_owned(),
                        serde_json::json!({
                            "thinkingConfig": {
                                "includeThoughts": true,
                            },
                        }),
                    );
            }
            body
        }
        ApiFamily::OllamaNative => serde_json::json!({
            "model": null,
            "messages": [],
            "stream": true,
        }),
    }
}

fn is_exact_built_in_openrouter(template: &ProviderTemplate) -> bool {
    template.source == TemplateSource::BuiltIn
        && template.id.as_str() == BuiltInTemplateId::OpenRouter.as_str()
        && template.manifest_version == BUILT_IN_TEMPLATE_VERSION
}

fn built_in_parameter_specs(family: ApiFamily) -> Vec<ParameterSpec> {
    let (temperature_field, temperature_maximum, output_tokens_field, output_tokens_default) =
        match family {
            ApiFamily::OpenAiResponses => (
                "temperature",
                Some(2.0),
                "max_output_tokens",
                ParameterDefaultMode::ProviderDefault,
            ),
            ApiFamily::OpenAiChatCompletions => (
                "temperature",
                Some(2.0),
                "max_tokens",
                ParameterDefaultMode::ProviderDefault,
            ),
            ApiFamily::AnthropicMessages => (
                "temperature",
                Some(1.0),
                "max_tokens",
                ParameterDefaultMode::ExplicitRequired,
            ),
            // Gemini publishes the upper temperature bound per model as
            // `maxTemperature`; the family contract may only enforce zero.
            ApiFamily::GeminiGenerateContent => (
                "generationConfig.temperature",
                None,
                "generationConfig.maxOutputTokens",
                ParameterDefaultMode::ProviderDefault,
            ),
            // Ollama options are model/runtime dependent, so the built-in
            // family contract likewise avoids inventing a maximum.
            ApiFamily::OllamaNative => (
                "options.temperature",
                None,
                "options.num_predict",
                ParameterDefaultMode::ProviderDefault,
            ),
        };

    let mut parameters = vec![
        number_parameter(
            "temperature",
            temperature_field,
            0.0,
            temperature_maximum,
            UiParameterLevel::Basic,
        ),
        integer_parameter(
            "max_output_tokens",
            output_tokens_field,
            1.0,
            Some(MAX_GENERATION_TOKENS),
            output_tokens_default,
            UiParameterLevel::Basic,
        ),
    ];

    // Anthropic has deprecated sampling controls for newer Messages models.
    // Its mandatory temperature entry above remains for the common UI
    // contract, but top-p is deliberately not advertised at family scope.
    if family != ApiFamily::AnthropicMessages {
        parameters.push(number_parameter(
            "top_p",
            match family {
                ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions => "top_p",
                ApiFamily::GeminiGenerateContent => "generationConfig.topP",
                ApiFamily::OllamaNative => "options.top_p",
                ApiFamily::AnthropicMessages => unreachable!("handled above"),
            },
            0.0,
            Some(1.0),
            UiParameterLevel::Advanced,
        ));
    }
    parameters
}

fn number_parameter(
    id: &str,
    field_name: &str,
    minimum: f64,
    maximum: Option<f64>,
    level: UiParameterLevel,
) -> ParameterSpec {
    parameter_spec(
        id,
        field_name,
        ParameterType::Number,
        Some(minimum),
        maximum,
        None,
        ParameterDefaultMode::ProviderDefault,
        level,
    )
}

fn integer_parameter(
    id: &str,
    field_name: &str,
    minimum: f64,
    maximum: Option<f64>,
    default_mode: ParameterDefaultMode,
    level: UiParameterLevel,
) -> ParameterSpec {
    parameter_spec(
        id,
        field_name,
        ParameterType::Integer,
        Some(minimum),
        maximum,
        Some(1.0),
        default_mode,
        level,
    )
}

#[allow(clippy::too_many_arguments)]
fn parameter_spec(
    id: &str,
    field_name: &str,
    value_type: ParameterType,
    minimum: Option<f64>,
    maximum: Option<f64>,
    step: Option<f64>,
    default_mode: ParameterDefaultMode,
    level: UiParameterLevel,
) -> ParameterSpec {
    ParameterSpec {
        id: ParameterId::from(id),
        label_key: format!("provider.parameter.{id}"),
        description_key: Some(format!("provider.parameter.{id}.description")),
        value_type,
        allowed_values: Vec::new(),
        minimum,
        maximum,
        step,
        default_mode,
        visibility: None,
        conflicts: Vec::new(),
        provider_mapping: ProviderParameterMapping {
            target: ProviderParameterTarget::RequestBody,
            field_name: field_name.to_owned(),
        },
        level,
    }
}

fn validate_credential_scope(
    connection: &ProviderConnection,
    auth: &AuthBinding,
) -> CoreResult<()> {
    match auth {
        AuthBinding::None => {
            if connection.credential_scope.is_some() {
                return Err(CoreError::invalid(
                    "credential-free provider connection must not carry a credential scope",
                ));
            }
        }
        AuthBinding::BearerHeader | AuthBinding::HeaderApiKey { .. } => {
            let scope = connection.credential_scope.as_ref().ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "credential origin approval is required for this provider connection",
                    false,
                )
            })?;
            if &scope.auth_binding != auth
                || scope.allowed_origins.len() != 1
                || scope.allowed_origins.first() != Some(&connection.api_origin)
                || !matches!(
                    scope.redirect_policy,
                    CredentialRedirectPolicy::Deny
                        | CredentialRedirectPolicy::FollowWithoutCredential
                )
            {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "provider credential scope does not match the adapter origin and auth binding",
                    false,
                ));
            }
        }
    }
    Ok(())
}

fn join_api_endpoint(root: &Url, endpoint: &EndpointPath) -> CoreResult<Url> {
    let root_path = root.path().trim_end_matches('/');
    let relative_path = endpoint.as_str();
    let joined_path = if root_path.is_empty() || root_path == "/" {
        relative_path.to_owned()
    } else {
        format!("{root_path}{relative_path}")
    };
    endpoint_path(&joined_path)?;
    let mut url = root.clone();
    url.set_path(&joined_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn api_key_field() -> ConnectionFieldSpec {
    ConnectionFieldSpec {
        key: "api_key".to_owned(),
        label_key: "provider.connection.api_key".to_owned(),
        description_key: Some("provider.connection.api_key.description".to_owned()),
        value_type: ConnectionFieldType::Credential,
        required: true,
    }
}

fn api_base_url_field(required: bool) -> ConnectionFieldSpec {
    ConnectionFieldSpec {
        key: "api_base_url".to_owned(),
        label_key: "provider.connection.api_base_url".to_owned(),
        description_key: Some("provider.connection.api_base_url.description".to_owned()),
        value_type: ConnectionFieldType::Text,
        required,
    }
}

fn manifest_source(kind: ManifestSourceKind, value: &str) -> CoreResult<ManifestSource> {
    Ok(ManifestSource {
        kind,
        url: HttpUrl::parse(value).map_err(|error| {
            CoreError::internal(format!("built-in provider source URL is invalid: {error}"))
        })?,
        content_sha256: None,
    })
}

fn canonical_origin(value: &str) -> CoreResult<CanonicalOrigin> {
    CanonicalOrigin::parse(value).map_err(|error| {
        CoreError::internal(format!("built-in provider origin is invalid: {error}"))
    })
}

fn endpoint_path(value: &str) -> CoreResult<EndpointPath> {
    EndpointPath::parse(value).map_err(|error| {
        CoreError::internal(format!("built-in provider endpoint is invalid: {error}"))
    })
}

fn domain_header_name(value: &str) -> CoreResult<DomainHeaderName> {
    DomainHeaderName::parse(value).map_err(|error| {
        CoreError::internal(format!("built-in provider auth header is invalid: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc,
        thread,
        time::Duration,
    };

    use lorepia_domain::{
        ApiFamily, CanonicalOrigin, ConnectionConfig, ConnectionStatus, ConversationId,
        CredentialRef, CredentialScope, GenerationId, GenerationPresetId,
        GenerationProviderProvenance, GenerationRequest, HeaderName, ModelAvailability, ModelRoute,
        ModelRouteConfig, ModelRouteId, ParameterDefaultMode, ParameterId, ParameterLiteral,
        ParameterSpec, ParameterType, ParameterValue, ParameterValueState, ProviderConnectionId,
        ProviderLocalNetworkApproval, ProviderNetworkMode, TemplateSource, UiParameterLevel,
    };
    use tokio::sync::{mpsc as tokio_mpsc, watch};

    use crate::parameter_mapping::{
        OpenRouterReasoningWireStyle, ParameterEngine, ParameterIssueCode, PromptCacheDirective,
        PromptCacheSettings, PromptCacheWireDialect, ProviderRequestPlan, ReasoningEffort,
        ReasoningMode, ReasoningSettings, ReasoningWireDialect, build_provider_request_plan,
    };
    use crate::request_plan::{RequestBodyField, RequestBodyShape};
    use crate::url_policy::{ApprovedLocalNetworkOrigin, UrlPolicy};
    use crate::{
        EmbeddingFailure, EmbeddingPurpose, EmbeddingRequest, EmbeddingRunOutcome,
        EmbeddingUnknownOutcomeReason,
    };

    use super::{
        AdapterRegistry, AuthBinding, BuiltInTemplateId, CoreErrorCode, CredentialRedirectPolicy,
        EndpointPath, EndpointSpec, HttpMethod, MAX_GENERATION_TOKENS, MAX_MODEL_ID_BYTES,
        ModelListBudget, ModelListRequest, ModelListSupport, ModelRecordSource,
        OpenRouterReasoningEffort, OpenRouterReasoningEffortSupport, OpenRouterSupportedParameter,
        OpenRouterSupportedParameterSupport, ProviderConnection, ProviderTemplate,
        canonical_origin, parse_model_page, parse_openrouter_models,
    };

    const SYNTHETIC_CREDENTIAL: &str = "synthetic-test-credential";

    struct FixtureResponse {
        status: &'static str,
        headers: Vec<(&'static str, &'static str)>,
        body: String,
    }

    fn json_response(body: &str) -> FixtureResponse {
        FixtureResponse {
            status: "200 OK",
            headers: vec![("Content-Type", "application/json")],
            body: body.to_owned(),
        }
    }

    fn fixture_server(
        responses: Vec<FixtureResponse>,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let (request_sender, request_receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept fixture request");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("request read timeout");
                let request = read_request(&mut stream);
                request_sender
                    .send(request)
                    .expect("record fixture request");
                let mut headers = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status,
                    response.body.len()
                );
                for (name, value) in response.headers {
                    headers.push_str(name);
                    headers.push_str(": ");
                    headers.push_str(value);
                    headers.push_str("\r\n");
                }
                headers.push_str("\r\n");
                stream
                    .write_all(headers.as_bytes())
                    .expect("write fixture headers");
                stream
                    .write_all(response.body.as_bytes())
                    .expect("write fixture body");
            }
        });
        (format!("http://{address}"), request_receiver, handle)
    }

    fn stalling_server() -> (
        String,
        tokio::sync::oneshot::Receiver<String>,
        mpsc::Sender<()>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stalling server");
        let address = listener.local_addr().expect("stalling server address");
        let (request_sender, request_receiver) = tokio::sync::oneshot::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept stalling request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("stalling request read timeout");
            let request = read_request(&mut stream);
            request_sender
                .send(request)
                .expect("record stalling request");
            release_receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("release stalling request");
        });
        (
            format!("http://{address}"),
            request_receiver,
            release_sender,
            handle,
        )
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 1_024];
        while bytes.len() <= 64 * 1_024 {
            let count = stream.read(&mut chunk).expect("read fixture request");
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(header_end) = bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
            {
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if bytes.len() >= header_end.saturating_add(content_length) {
                    break;
                }
            }
        }
        String::from_utf8(bytes).expect("fixture request is UTF-8")
    }

    fn connection_for(template: &ProviderTemplate, origin: &str) -> ProviderConnection {
        let api_origin = canonical_origin(origin).expect("fixture origin");
        let credential_scope = match &template.default_manifest.auth {
            AuthBinding::None => None,
            auth => Some(CredentialScope {
                allowed_origins: vec![api_origin.clone()],
                auth_binding: auth.clone(),
                redirect_policy: CredentialRedirectPolicy::Deny,
            }),
        };
        let network_mode = if origin.starts_with("http://127.0.0.1:")
            || origin.starts_with("https://127.0.0.1:")
            || origin.starts_with("http://localhost:")
            || origin.starts_with("https://localhost:")
        {
            ProviderNetworkMode::LocalLoopback
        } else {
            ProviderNetworkMode::Public
        };
        let api_base_path = if template.source == TemplateSource::BuiltIn {
            BuiltInTemplateId::ALL
                .into_iter()
                .find(|id| id.as_str() == template.id.as_str())
                .map(|id| {
                    EndpointPath::parse(id.default_api_base_path())
                        .expect("built-in base path is valid")
                })
        } else {
            None
        };
        ProviderConnection {
            id: ProviderConnectionId::from("connection-1"),
            template_id: template.id.clone(),
            template_version: template.manifest_version,
            display_name: "Synthetic connection".to_owned(),
            api_origin,
            config: ConnectionConfig {
                api_base_path,
                network_mode,
                local_network_approval: None,
                values: Vec::new(),
            },
            credential_ref: credential_scope
                .as_ref()
                .map(|_| CredentialRef("credential-1".to_owned())),
            credential_scope,
            timeout_seconds: 5,
            status: ConnectionStatus::Untested,
            created_at: "2026-07-31T00:00:00Z".parse().expect("fixture timestamp"),
            updated_at: "2026-07-31T00:00:00Z".parse().expect("fixture timestamp"),
        }
    }

    fn custom_openai_template() -> ProviderTemplate {
        let mut template =
            AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
                .expect("OpenAI-compatible template");
        template.id = lorepia_domain::ProviderTemplateId::from("custom-openai-fixture");
        template.manifest_version = 7;
        template.source = TemplateSource::UserDiscovered;
        template.default_manifest.default_api_origin = None;
        template.default_manifest.auth = AuthBinding::HeaderApiKey {
            header_name: HeaderName::parse("x-fixture-key").expect("fixture header"),
        };
        template.default_manifest.endpoints.models = Some(EndpointSpec {
            method: HttpMethod::Get,
            path: EndpointPath::parse("/tenant/catalog").expect("fixture model path"),
        });
        template.default_manifest.endpoints.generate = EndpointSpec {
            method: HttpMethod::Post,
            path: EndpointPath::parse("/tenant/generate").expect("fixture generation path"),
        };
        template
    }

    fn generation_request(model: &str) -> GenerationRequest {
        GenerationRequest {
            generation_id: GenerationId::new(),
            conversation_id: ConversationId::new(),
            model: model.to_owned(),
            messages: Vec::new(),
            resolved_prompt_plan: None,
            provider_execution_plan_hash: None,
            temperature: None,
            max_output_tokens: None,
            provider_provenance: None,
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        }
    }

    fn route_for(connection: &ProviderConnection, model_id: &str) -> ModelRoute {
        ModelRoute {
            id: ModelRouteId::from("route-fixture"),
            connection_id: connection.id.clone(),
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: model_id.to_owned(),
            display_name: None,
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: lorepia_domain::ModelMetadataSource::Legacy,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: "2026-07-31T00:00:00Z".parse().expect("fixture timestamp"),
            last_seen_at: None,
        }
    }

    fn route_for_template(
        template: &ProviderTemplate,
        connection: &ProviderConnection,
        model_id: &str,
    ) -> ModelRoute {
        let mut route = route_for(connection, model_id);
        route.api_family = template.api_family;
        route
    }

    fn preview_top_level_fields(preview: &crate::RequestPreview) -> Vec<&str> {
        let Some(RequestBodyShape::Object { fields, .. }) = preview.body() else {
            panic!("provider preview body must be an object");
        };
        fields.iter().map(RequestBodyField::name).collect()
    }

    #[test]
    fn built_in_templates_cover_the_closed_adapter_allowlist() {
        let templates = AdapterRegistry::built_in_templates().expect("built-in templates");
        assert_eq!(templates.len(), BuiltInTemplateId::ALL.len());
        for (id, template) in BuiltInTemplateId::ALL.into_iter().zip(&templates) {
            assert_eq!(template.id.as_str(), id.as_str());
            assert_eq!(template.api_family, id.family());
            ParameterEngine::from_manifest_specs(&template.default_manifest.parameters)
                .expect("built-in parameter contract");
            assert!(
                template.default_manifest.parameters.len() >= 2,
                "{id:?} must expose generation controls"
            );
        }

        let unknown = AdapterRegistry::family_from_wire("generic_json")
            .expect_err("unknown adapter must fail closed");
        assert_eq!(unknown.code, CoreErrorCode::UnsupportedContent);
        assert_eq!(
            AdapterRegistry::family_from_wire("openai_responses").expect("known family"),
            lorepia_domain::ApiFamily::OpenAiResponses
        );
    }

    #[test]
    fn opaque_reasoning_continuity_is_not_advertised_by_templates() {
        for (id, supported) in [
            (BuiltInTemplateId::OpenAiResponses, false),
            (BuiltInTemplateId::AnthropicMessages, false),
            (BuiltInTemplateId::OpenRouter, false),
            (BuiltInTemplateId::OpenAiChatCompatible, false),
            (BuiltInTemplateId::GeminiGenerateContent, false),
            (BuiltInTemplateId::OllamaNative, false),
        ] {
            let template = AdapterRegistry::built_in_template(id).expect("built-in template");
            assert_eq!(
                AdapterRegistry::template_supports_opaque_reasoning_state(&template),
                supported,
                "{id:?}"
            );
        }

        let mut spoofed =
            AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter).expect("OpenRouter");
        spoofed.source = TemplateSource::UserDiscovered;
        assert!(
            !AdapterRegistry::template_supports_opaque_reasoning_state(&spoofed),
            "a user template cannot advertise disabled opaque continuity by reusing a built-in ID"
        );

        let mut tampered =
            AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter).expect("OpenRouter");
        tampered.default_manifest.endpoints.generate.path =
            EndpointPath::parse("/different/chat/completions").expect("different endpoint");
        assert!(
            !AdapterRegistry::template_supports_opaque_reasoning_state(&tampered),
            "a mutated built-in endpoint is not the exact continuity-capable template"
        );

        let mut stale = AdapterRegistry::built_in_template(BuiltInTemplateId::AnthropicMessages)
            .expect("Anthropic");
        stale.manifest_version = stale.manifest_version.saturating_sub(1);
        assert!(!AdapterRegistry::template_supports_opaque_reasoning_state(
            &stale
        ));
    }

    #[test]
    fn built_in_parameters_use_provider_native_fields_and_safe_bounds() {
        let expected = [
            (
                BuiltInTemplateId::OpenAiResponses,
                "temperature",
                Some(2.0),
                "max_output_tokens",
                true,
            ),
            (
                BuiltInTemplateId::OpenAiChatCompatible,
                "temperature",
                Some(2.0),
                "max_tokens",
                true,
            ),
            (
                BuiltInTemplateId::AnthropicMessages,
                "temperature",
                Some(1.0),
                "max_tokens",
                false,
            ),
            (
                BuiltInTemplateId::GeminiGenerateContent,
                "generationConfig.temperature",
                None,
                "generationConfig.maxOutputTokens",
                true,
            ),
            (
                BuiltInTemplateId::OpenRouter,
                "temperature",
                Some(2.0),
                "max_tokens",
                true,
            ),
            (
                BuiltInTemplateId::OllamaNative,
                "options.temperature",
                None,
                "options.num_predict",
                true,
            ),
        ];

        for (id, temperature_field, temperature_maximum, output_field, has_top_p) in expected {
            let template = AdapterRegistry::built_in_template(id).expect("built-in template");
            let parameters = &template.default_manifest.parameters;
            let temperature = parameter(parameters, "temperature");
            assert_eq!(temperature.value_type, ParameterType::Number);
            assert_eq!(temperature.minimum, Some(0.0));
            assert_eq!(temperature.maximum, temperature_maximum);
            assert_eq!(temperature.step, None);
            assert_eq!(
                temperature.default_mode,
                ParameterDefaultMode::ProviderDefault
            );
            assert_eq!(temperature.level, UiParameterLevel::Basic);
            assert_eq!(
                temperature.provider_mapping.field_name, temperature_field,
                "{id:?}"
            );

            let output_tokens = parameter(parameters, "max_output_tokens");
            assert_eq!(output_tokens.value_type, ParameterType::Integer);
            assert_eq!(output_tokens.minimum, Some(1.0));
            assert_eq!(output_tokens.maximum, Some(MAX_GENERATION_TOKENS));
            assert_eq!(output_tokens.step, Some(1.0));
            assert_eq!(output_tokens.level, UiParameterLevel::Basic);
            assert_eq!(
                output_tokens.provider_mapping.field_name, output_field,
                "{id:?}"
            );
            assert_eq!(
                output_tokens.default_mode,
                if id == BuiltInTemplateId::AnthropicMessages {
                    ParameterDefaultMode::ExplicitRequired
                } else {
                    ParameterDefaultMode::ProviderDefault
                }
            );

            let top_p = parameters
                .iter()
                .find(|specification| specification.id.as_str() == "top_p");
            assert_eq!(top_p.is_some(), has_top_p, "{id:?}");
            if let Some(top_p) = top_p {
                assert_eq!(top_p.value_type, ParameterType::Number);
                assert_eq!((top_p.minimum, top_p.maximum), (Some(0.0), Some(1.0)));
                assert_eq!(top_p.default_mode, ParameterDefaultMode::ProviderDefault);
                assert_eq!(top_p.level, UiParameterLevel::Advanced);
            }
        }
    }

    #[test]
    fn untouched_optional_built_in_parameters_are_omitted_from_request_plans() {
        for id in [
            BuiltInTemplateId::OpenAiResponses,
            BuiltInTemplateId::OpenAiChatCompatible,
            BuiltInTemplateId::GeminiGenerateContent,
            BuiltInTemplateId::OpenRouter,
            BuiltInTemplateId::OllamaNative,
        ] {
            let template = AdapterRegistry::built_in_template(id).expect("built-in template");
            let engine =
                ParameterEngine::from_manifest_specs(&template.default_manifest.parameters)
                    .expect("parameter contract");
            let validated = engine
                .validate_for_request(&[])
                .expect("all parameters inherit provider defaults");
            assert!(validated.applied().is_empty(), "{id:?}");
            assert_eq!(
                validated.omitted_provider_defaults().len(),
                template.default_manifest.parameters.len(),
                "{id:?}"
            );

            let plan = build_provider_request_plan(
                template.api_family,
                &validated,
                &default_reasoning_for(&template),
                &ReasoningWireDialect::Unsupported,
                &PromptCacheSettings::default(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect("empty provider-default request plan");
            assert!(plan.body_patches.is_empty(), "{id:?}");
        }

        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::AnthropicMessages)
            .expect("Anthropic template");
        let engine = ParameterEngine::from_manifest_specs(&template.default_manifest.parameters)
            .expect("Anthropic parameter contract");
        let validated = engine
            .validate_for_request(&[explicit_integer("max_output_tokens", 512)])
            .expect("required token limit with untouched optional controls");
        assert_eq!(validated.applied().len(), 1);
        assert_eq!(validated.omitted_provider_defaults().len(), 1);
        let plan = build_provider_request_plan(
            ApiFamily::AnthropicMessages,
            &validated,
            &ReasoningSettings::default(),
            &ReasoningWireDialect::Unsupported,
            &PromptCacheSettings::default(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("Anthropic request plan");
        assert_eq!(plan.body_patches.len(), 1);
        assert_eq!(plan.body_patches[0].path, "max_tokens");
        assert_eq!(plan.body_patches[0].value, serde_json::json!(512));
    }

    #[test]
    fn anthropic_requires_an_explicit_positive_max_tokens_value() {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::AnthropicMessages)
            .expect("Anthropic template");
        let engine = ParameterEngine::from_manifest_specs(&template.default_manifest.parameters)
            .expect("Anthropic parameter contract");

        let missing = engine
            .validate_for_request(&[])
            .expect_err("missing max tokens must fail");
        assert!(missing.issues.iter().any(|issue| {
            issue.code == ParameterIssueCode::RequiredValueMissing
                && issue.parameter_id.as_ref().map(ParameterId::as_str) == Some("max_output_tokens")
        }));

        let zero = engine
            .validate_for_request(&[explicit_integer("max_output_tokens", 0)])
            .expect_err("zero max tokens must fail");
        assert!(zero.issues.iter().any(|issue| {
            issue.code == ParameterIssueCode::OutOfBounds
                && issue.parameter_id.as_ref().map(ParameterId::as_str) == Some("max_output_tokens")
        }));

        engine
            .validate_for_request(&[explicit_integer("max_output_tokens", 1)])
            .expect("positive max tokens");
    }

    #[test]
    fn every_built_in_parameter_maps_through_the_closed_family_allowlist() {
        for id in BuiltInTemplateId::ALL {
            let template = AdapterRegistry::built_in_template(id).expect("built-in template");
            let engine =
                ParameterEngine::from_manifest_specs(&template.default_manifest.parameters)
                    .expect("parameter contract");
            let values = template
                .default_manifest
                .parameters
                .iter()
                .map(|specification| ParameterValue {
                    parameter_id: specification.id.clone(),
                    state: ParameterValueState::Explicit(match specification.value_type {
                        ParameterType::Integer => ParameterLiteral::Integer(256),
                        ParameterType::Number => ParameterLiteral::Number(0.5),
                        other => panic!("unexpected built-in parameter type: {other:?}"),
                    }),
                })
                .collect::<Vec<_>>();
            let validated = engine
                .validate_for_request(&values)
                .expect("valid explicit values");
            let plan = build_provider_request_plan(
                template.api_family,
                &validated,
                &default_reasoning_for(&template),
                &ReasoningWireDialect::Unsupported,
                &PromptCacheSettings::default(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect("all built-in fields must be allowlisted");
            assert_eq!(
                plan.body_patches.len(),
                template.default_manifest.parameters.len(),
                "{id:?}"
            );
        }
    }

    fn parameter<'a>(parameters: &'a [ParameterSpec], id: &str) -> &'a ParameterSpec {
        parameters
            .iter()
            .find(|specification| specification.id.as_str() == id)
            .unwrap_or_else(|| panic!("missing {id} parameter"))
    }

    fn explicit_integer(id: &str, value: i64) -> ParameterValue {
        ParameterValue {
            parameter_id: ParameterId::from(id),
            state: ParameterValueState::Explicit(ParameterLiteral::Integer(value)),
        }
    }

    fn default_reasoning_for(template: &ProviderTemplate) -> ReasoningSettings {
        ReasoningSettings {
            preserve_opaque_state: AdapterRegistry::template_supports_opaque_reasoning_state(
                template,
            ),
            ..ReasoningSettings::default()
        }
    }

    #[test]
    fn embedding_capability_uses_a_new_immutable_builtin_template_version() {
        let openai = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        assert_eq!(openai.manifest_version, 3);
    }

    #[test]
    fn built_in_templates_declare_safe_origins_and_endpoints() {
        let openai = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        assert_eq!(
            openai
                .default_manifest
                .default_api_origin
                .as_ref()
                .expect("OpenAI origin")
                .as_str(),
            "https://api.openai.com"
        );
        assert_eq!(
            openai
                .default_manifest
                .endpoints
                .models
                .as_ref()
                .expect("models endpoint")
                .path
                .as_str(),
            "/models"
        );

        let openrouter = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("OpenRouter template");
        assert_eq!(
            openrouter.api_family,
            lorepia_domain::ApiFamily::OpenAiChatCompletions
        );
        assert_eq!(
            openrouter
                .default_manifest
                .default_api_origin
                .as_ref()
                .expect("OpenRouter origin")
                .as_str(),
            "https://openrouter.ai"
        );
        assert_eq!(
            BuiltInTemplateId::OpenRouter.default_api_base_path(),
            "/api/v1"
        );
        assert_eq!(openrouter.default_manifest.auth, AuthBinding::BearerHeader);
        assert_eq!(
            openrouter
                .default_manifest
                .endpoints
                .models
                .as_ref()
                .expect("OpenRouter models endpoint")
                .path
                .as_str(),
            "/models"
        );
        assert_eq!(
            openrouter.default_manifest.endpoints.generate.path.as_str(),
            "/chat/completions"
        );

        let ollama = AdapterRegistry::built_in_template(BuiltInTemplateId::OllamaNative)
            .expect("Ollama template");
        assert_eq!(
            ollama
                .default_manifest
                .default_api_origin
                .as_ref()
                .expect("Ollama origin")
                .as_str(),
            "http://localhost:11434"
        );
        assert_eq!(ollama.default_manifest.auth, AuthBinding::None);
        assert_eq!(
            AdapterRegistry::descriptor(ollama.api_family)
                .expect("Ollama descriptor")
                .default_network_mode,
            ProviderNetworkMode::LocalLoopback
        );
        assert_eq!(
            openai
                .default_manifest
                .endpoints
                .embeddings
                .as_ref()
                .expect("OpenAI embeddings endpoint")
                .path
                .as_str(),
            "/embeddings"
        );
        assert_eq!(
            ollama
                .default_manifest
                .endpoints
                .embeddings
                .as_ref()
                .expect("Ollama embeddings endpoint")
                .path
                .as_str(),
            "/embed"
        );
        let anthropic = AdapterRegistry::built_in_template(BuiltInTemplateId::AnthropicMessages)
            .expect("Anthropic template");
        assert!(anthropic.default_manifest.endpoints.embeddings.is_none());
    }

    #[tokio::test]
    async fn openai_embedding_dispatch_is_exact_and_credential_is_header_only() {
        let (origin, requests, server) = fixture_server(vec![json_response(
            r#"{"data":[{"embedding":[0.25,-0.5,0.75]}],"model":"text-embedding-fixture"}"#,
        )]);
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        let connection = connection_for(&template, &origin);
        let route = route_for_template(&template, &connection, "text-embedding-fixture");
        let provider = AdapterRegistry::new()
            .build_embedding_provider_for_route(&template, &connection, &route, 3)
            .expect("OpenAI embedding provider");
        let contract_sha256 = provider
            .contract()
            .execution_sha256(EmbeddingPurpose::RetrievalDocument);
        assert_eq!(contract_sha256.len(), 64);
        assert_ne!(
            contract_sha256,
            provider
                .contract()
                .execution_sha256(EmbeddingPurpose::RetrievalQuery)
        );
        let (_cancel_sender, cancelled) = watch::channel(false);
        let outcome = provider
            .embed(
                EmbeddingRequest::new(
                    "text-embedding-fixture",
                    "bounded document",
                    3,
                    EmbeddingPurpose::RetrievalDocument,
                )
                .expect("embedding request"),
                Some(SYNTHETIC_CREDENTIAL),
                cancelled,
            )
            .await;
        assert!(!format!("{outcome:?}").contains(SYNTHETIC_CREDENTIAL));
        assert!(!format!("{:?}", provider.contract()).contains(SYNTHETIC_CREDENTIAL));
        let EmbeddingRunOutcome::Completed(output) = outcome else {
            panic!("expected completed embedding");
        };
        assert_eq!(output.values(), &[0.25, -0.5, 0.75]);

        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("record OpenAI embedding request");
        assert!(request.starts_with("POST /v1/embeddings HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-test-credential\r\n")
        );
        let (_, body) = request.split_once("\r\n\r\n").expect("request body");
        let body: serde_json::Value = serde_json::from_str(body).expect("JSON request");
        assert_eq!(
            body,
            serde_json::json!({
                "model": "text-embedding-fixture",
                "input": "bounded document",
                "dimensions": 3,
                "encoding_format": "float",
            })
        );
        assert!(!body.to_string().contains(SYNTHETIC_CREDENTIAL));
        server.join().expect("OpenAI embedding fixture server");
    }

    #[tokio::test]
    async fn openrouter_embedding_disables_provider_fallbacks() {
        let (origin, requests, server) = fixture_server(vec![json_response(
            r#"{"data":[{"embedding":[1,2,3]}],"model":"openai/text-embedding-3-small"}"#,
        )]);
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("OpenRouter template");
        let connection = connection_for(&template, &origin);
        let route = route_for_template(&template, &connection, "openai/text-embedding-3-small");
        let provider = AdapterRegistry::new()
            .build_embedding_provider_for_route(&template, &connection, &route, 3)
            .expect("OpenRouter embedding provider");
        let (_cancel_sender, cancelled) = watch::channel(false);
        let outcome = provider
            .embed(
                EmbeddingRequest::new(
                    "openai/text-embedding-3-small",
                    "bounded document",
                    3,
                    EmbeddingPurpose::RetrievalDocument,
                )
                .expect("embedding request"),
                Some(SYNTHETIC_CREDENTIAL),
                cancelled,
            )
            .await;
        assert!(matches!(outcome, EmbeddingRunOutcome::Completed(_)));

        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("record OpenRouter embedding request");
        assert!(request.starts_with("POST /api/v1/embeddings HTTP/1.1\r\n"));
        let (_, body) = request.split_once("\r\n\r\n").expect("request body");
        let body: serde_json::Value = serde_json::from_str(body).expect("JSON request");
        assert_eq!(body["provider"]["allow_fallbacks"], false);
        assert_eq!(body["provider"]["require_parameters"], true);
        assert!(body.get("models").is_none());
        server.join().expect("OpenRouter embedding fixture server");
    }

    #[tokio::test]
    async fn gemini_embedding_dispatch_binds_model_purpose_and_dimensions() {
        let (origin, requests, server) =
            fixture_server(vec![json_response(r#"{"embedding":{"values":[1,2,3]}}"#)]);
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::GeminiGenerateContent)
            .expect("Gemini template");
        let connection = connection_for(&template, &origin);
        let route = route_for_template(&template, &connection, "text-embedding-fixture");
        let provider = AdapterRegistry::new()
            .build_embedding_provider_for_route(&template, &connection, &route, 3)
            .expect("Gemini embedding provider");
        let (_cancel_sender, cancelled) = watch::channel(false);
        let outcome = provider
            .embed(
                EmbeddingRequest::new(
                    "text-embedding-fixture",
                    "bounded query",
                    3,
                    EmbeddingPurpose::RetrievalQuery,
                )
                .expect("embedding request"),
                Some(SYNTHETIC_CREDENTIAL),
                cancelled,
            )
            .await;
        assert!(matches!(outcome, EmbeddingRunOutcome::Completed(_)));

        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("record Gemini embedding request");
        assert!(
            request.starts_with(
                "POST /v1beta/models/text-embedding-fixture:embedContent HTTP/1.1\r\n"
            )
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("x-goog-api-key: synthetic-test-credential\r\n")
        );
        let (_, body) = request.split_once("\r\n\r\n").expect("request body");
        let body: serde_json::Value = serde_json::from_str(body).expect("JSON request");
        assert_eq!(body["model"], "models/text-embedding-fixture");
        assert_eq!(body["content"]["parts"][0]["text"], "bounded query");
        assert_eq!(body["embedContentConfig"]["taskType"], "RETRIEVAL_QUERY");
        assert_eq!(body["embedContentConfig"]["autoTruncate"], false);
        assert_eq!(body["embedContentConfig"]["outputDimensionality"], 3);
        assert!(!body.to_string().contains(SYNTHETIC_CREDENTIAL));
        server.join().expect("Gemini embedding fixture server");
    }

    #[tokio::test]
    async fn ollama_embedding_dispatch_uses_closed_native_schema() {
        let (origin, requests, server) = fixture_server(vec![json_response(
            r#"{"model":"nomic-embed-text","embeddings":[[1,2,3]]}"#,
        )]);
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OllamaNative)
            .expect("Ollama template");
        let connection = connection_for(&template, &origin);
        let route = route_for_template(&template, &connection, "nomic-embed-text");
        let provider = AdapterRegistry::new()
            .build_embedding_provider_for_route(&template, &connection, &route, 3)
            .expect("Ollama embedding provider");
        let (_cancel_sender, cancelled) = watch::channel(false);
        let outcome = provider
            .embed(
                EmbeddingRequest::new(
                    "nomic-embed-text",
                    "bounded document",
                    3,
                    EmbeddingPurpose::RetrievalDocument,
                )
                .expect("embedding request"),
                None,
                cancelled,
            )
            .await;
        assert!(matches!(outcome, EmbeddingRunOutcome::Completed(_)));

        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("record Ollama embedding request");
        assert!(request.starts_with("POST /api/embed HTTP/1.1\r\n"));
        let (_, body) = request.split_once("\r\n\r\n").expect("request body");
        let body: serde_json::Value = serde_json::from_str(body).expect("JSON request");
        assert_eq!(
            body,
            serde_json::json!({
                "model": "nomic-embed-text",
                "input": "bounded document",
                "truncate": false,
                "dimensions": 3,
            })
        );
        server.join().expect("Ollama embedding fixture server");
    }

    #[test]
    fn anthropic_and_missing_embedding_capabilities_fail_before_dispatch() {
        let anthropic = AdapterRegistry::built_in_template(BuiltInTemplateId::AnthropicMessages)
            .expect("Anthropic template");
        let connection = connection_for(&anthropic, "http://127.0.0.1:9");
        let route = route_for_template(&anthropic, &connection, "claude-fixture");
        let error = AdapterRegistry::new()
            .build_embedding_provider_for_route(&anthropic, &connection, &route, 3)
            .err()
            .expect("Anthropic embeddings must fail");
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);

        let mut openai = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        openai.default_manifest.endpoints.embeddings = None;
        let connection = connection_for(&openai, "http://127.0.0.1:9");
        let route = route_for_template(&openai, &connection, "embedding-fixture");
        let error = AdapterRegistry::new()
            .build_embedding_provider_for_route(&openai, &connection, &route, 3)
            .err()
            .expect("missing manifest capability must fail");
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
    }

    #[tokio::test]
    async fn credential_canary_and_pre_dispatch_cancellation_never_reach_network() {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        let connection = connection_for(&template, "http://127.0.0.1:9");
        let route = route_for_template(&template, &connection, "embedding-fixture");
        let provider = AdapterRegistry::new()
            .build_embedding_provider_for_route(&template, &connection, &route, 3)
            .expect("embedding provider");

        let (_cancel_sender, cancelled) = watch::channel(false);
        let canary = provider
            .embed(
                EmbeddingRequest::new(
                    "embedding-fixture",
                    format!("document contains {SYNTHETIC_CREDENTIAL}"),
                    3,
                    EmbeddingPurpose::RetrievalDocument,
                )
                .expect("canary request"),
                Some(SYNTHETIC_CREDENTIAL),
                cancelled,
            )
            .await;
        assert_eq!(
            canary,
            EmbeddingRunOutcome::Failed(EmbeddingFailure::InvalidRequest)
        );
        assert!(!format!("{canary:?}").contains(SYNTHETIC_CREDENTIAL));

        let (_cancel_sender, cancelled) = watch::channel(false);
        let wrong_space = provider
            .embed(
                EmbeddingRequest::new(
                    "different-embedding-model",
                    "bounded document",
                    2,
                    EmbeddingPurpose::RetrievalDocument,
                )
                .expect("mismatched request"),
                Some(SYNTHETIC_CREDENTIAL),
                cancelled,
            )
            .await;
        assert_eq!(
            wrong_space,
            EmbeddingRunOutcome::Failed(EmbeddingFailure::InvalidRequest)
        );

        let (cancel_sender, cancelled) = watch::channel(true);
        let cancelled_outcome = provider
            .embed(
                EmbeddingRequest::new(
                    "embedding-fixture",
                    "bounded document",
                    3,
                    EmbeddingPurpose::RetrievalDocument,
                )
                .expect("cancelled request"),
                Some(SYNTHETIC_CREDENTIAL),
                cancelled,
            )
            .await;
        assert_eq!(
            cancelled_outcome,
            EmbeddingRunOutcome::CancelledBeforeDispatch
        );
        drop(cancel_sender);
    }

    #[tokio::test]
    async fn cancellation_after_dispatch_is_an_exact_unknown_outcome() {
        let (origin, request_seen, release_server, server) = stalling_server();
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        let connection = connection_for(&template, &origin);
        let route = route_for_template(&template, &connection, "embedding-fixture");
        let provider = AdapterRegistry::new()
            .build_embedding_provider_for_route(&template, &connection, &route, 3)
            .expect("embedding provider");
        let expected_contract = provider
            .contract()
            .execution_sha256(EmbeddingPurpose::RetrievalDocument);
        let (cancel_sender, cancelled) = watch::channel(false);
        let task = tokio::spawn(async move {
            provider
                .embed(
                    EmbeddingRequest::new(
                        "embedding-fixture",
                        "bounded document",
                        3,
                        EmbeddingPurpose::RetrievalDocument,
                    )
                    .expect("embedding request"),
                    Some(SYNTHETIC_CREDENTIAL),
                    cancelled,
                )
                .await
        });
        let request = request_seen.await.expect("request reached fixture");
        assert!(request.starts_with("POST /v1/embeddings HTTP/1.1\r\n"));
        cancel_sender.send(true).expect("cancel dispatched request");
        let outcome = task.await.expect("embedding task");
        let EmbeddingRunOutcome::UnknownOutcome(unknown) = outcome else {
            panic!("post-dispatch cancellation must be unknown");
        };
        assert_eq!(
            unknown.reason(),
            EmbeddingUnknownOutcomeReason::CancelledAfterDispatch
        );
        assert_eq!(unknown.contract_sha256(), expected_contract);
        release_server.send(()).expect("release fixture server");
        server.join().expect("stalling fixture server");
    }

    #[tokio::test]
    async fn timeout_after_dispatch_is_an_exact_unknown_outcome() {
        let (origin, request_seen, release_server, server) = stalling_server();
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        let mut connection = connection_for(&template, &origin);
        connection.timeout_seconds = 1;
        let route = route_for_template(&template, &connection, "embedding-fixture");
        let provider = AdapterRegistry::new()
            .build_embedding_provider_for_route(&template, &connection, &route, 3)
            .expect("embedding provider");
        let (_cancel_sender, cancelled) = watch::channel(false);
        let task = tokio::spawn(async move {
            provider
                .embed(
                    EmbeddingRequest::new(
                        "embedding-fixture",
                        "bounded document",
                        3,
                        EmbeddingPurpose::RetrievalDocument,
                    )
                    .expect("embedding request"),
                    Some(SYNTHETIC_CREDENTIAL),
                    cancelled,
                )
                .await
        });
        request_seen.await.expect("request reached fixture");
        let outcome = task.await.expect("embedding task");
        let EmbeddingRunOutcome::UnknownOutcome(unknown) = outcome else {
            panic!("post-dispatch timeout must be unknown");
        };
        assert_eq!(
            unknown.reason(),
            EmbeddingUnknownOutcomeReason::TimedOutAfterDispatch
        );
        release_server.send(()).expect("release fixture server");
        server.join().expect("stalling fixture server");
    }

    #[tokio::test]
    async fn provider_4xx_is_a_known_rejection_not_an_unknown_outcome() {
        let (origin, _requests, server) = fixture_server(vec![FixtureResponse {
            status: "400 Bad Request",
            headers: vec![("Content-Type", "application/json")],
            body: r#"{"error":{"message":"synthetic rejection"}}"#.to_owned(),
        }]);
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        let connection = connection_for(&template, &origin);
        let route = route_for_template(&template, &connection, "embedding-fixture");
        let provider = AdapterRegistry::new()
            .build_embedding_provider_for_route(&template, &connection, &route, 3)
            .expect("embedding provider");
        let (_cancel_sender, cancelled) = watch::channel(false);
        let outcome = provider
            .embed(
                EmbeddingRequest::new(
                    "embedding-fixture",
                    "bounded document",
                    3,
                    EmbeddingPurpose::RetrievalDocument,
                )
                .expect("embedding request"),
                Some(SYNTHETIC_CREDENTIAL),
                cancelled,
            )
            .await;
        assert_eq!(
            outcome,
            EmbeddingRunOutcome::Failed(EmbeddingFailure::ProviderRejected)
        );
        server.join().expect("4xx fixture server");
    }

    #[test]
    fn gemini_model_capability_preflight_separates_generation_and_embedding_spaces() {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::GeminiGenerateContent)
            .expect("Gemini template");
        let connection = connection_for(&template, "http://127.0.0.1:9");

        let mut generation_only =
            route_for_template(&template, &connection, "gemini-generation-only");
        generation_only.raw_metadata = Some(
            lorepia_domain::BoundedJson::from_value(&serde_json::json!({
                "supported_generation_methods": ["generateContent"],
            }))
            .expect("bounded metadata"),
        );
        let error = AdapterRegistry::new()
            .build_embedding_provider_for_route(&template, &connection, &generation_only, 3)
            .err()
            .expect("generation-only Gemini route must reject embeddings");
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);

        let mut embedding_only =
            route_for_template(&template, &connection, "gemini-embedding-only");
        embedding_only.raw_metadata = Some(
            lorepia_domain::BoundedJson::from_value(&serde_json::json!({
                "supported_generation_methods": ["embedContent"],
            }))
            .expect("bounded metadata"),
        );
        AdapterRegistry::new()
            .build_embedding_provider_for_route(&template, &connection, &embedding_only, 3)
            .expect("embedding-only Gemini route supports embeddings");
        let error = AdapterRegistry::new()
            .build_provider_for_route_with_plan(&template, &connection, &embedding_only, None)
            .err()
            .expect("embedding-only Gemini route must reject generation");
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
    }

    #[test]
    fn openrouter_resolves_exact_default_and_user_overridden_endpoint_paths() {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("OpenRouter template");
        let mut connection = connection_for(&template, "https://openrouter.ai");

        let models = super::join_manifest_endpoint(
            &connection,
            &template
                .default_manifest
                .endpoints
                .models
                .as_ref()
                .expect("models endpoint")
                .path,
        )
        .expect("models endpoint");
        let generation = super::join_manifest_endpoint(
            &connection,
            &template.default_manifest.endpoints.generate.path,
        )
        .expect("generation endpoint");
        assert_eq!(models.as_str(), "https://openrouter.ai/api/v1/models");
        assert_eq!(
            generation.as_str(),
            "https://openrouter.ai/api/v1/chat/completions"
        );

        connection.config.api_base_path =
            Some(EndpointPath::parse("/approved/custom").expect("custom base path"));
        let models = super::join_manifest_endpoint(
            &connection,
            &template
                .default_manifest
                .endpoints
                .models
                .as_ref()
                .expect("models endpoint")
                .path,
        )
        .expect("custom models endpoint");
        let generation = super::join_manifest_endpoint(
            &connection,
            &template.default_manifest.endpoints.generate.path,
        )
        .expect("custom generation endpoint");
        assert_eq!(models.path(), "/approved/custom/models");
        assert_eq!(generation.path(), "/approved/custom/chat/completions");
    }

    #[test]
    fn built_in_and_legacy_base_paths_are_applied_exactly_once() {
        for id in BuiltInTemplateId::ALL {
            let template = AdapterRegistry::built_in_template(id).expect("built-in template");
            let origin = template
                .default_manifest
                .default_api_origin
                .as_ref()
                .map_or("https://api.example.test", CanonicalOrigin::as_str);
            let connection = connection_for(&template, origin);
            let generation = super::join_manifest_endpoint(
                &connection,
                &template.default_manifest.endpoints.generate.path,
            )
            .expect("generation endpoint");
            let expected_prefix = id.default_api_base_path().trim_end_matches('/');
            assert!(
                generation.path().starts_with(expected_prefix),
                "{id:?} must retain its reviewed base path"
            );
            assert!(
                !generation
                    .path()
                    .contains(&format!("{expected_prefix}{expected_prefix}")),
                "{id:?} must not duplicate its base path"
            );
        }

        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
            .expect("legacy-compatible template");
        let connection = connection_for(&template, "https://legacy.example.test");
        let generation = super::join_manifest_endpoint(
            &connection,
            &template.default_manifest.endpoints.generate.path,
        )
        .expect("legacy endpoint");
        assert_eq!(generation.path(), "/v1/chat/completions");
    }

    #[test]
    fn credential_scope_is_restricted_to_one_canonical_origin() {
        let registry = AdapterRegistry::new();
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("OpenRouter template");
        let mut connection = connection_for(&template, "https://openrouter.ai");
        connection
            .credential_scope
            .as_mut()
            .expect("credential scope")
            .allowed_origins
            .push(canonical_origin("https://example.com").expect("second origin"));

        let error = registry
            .build_provider(&template, &connection)
            .err()
            .expect("multi-origin credential scope must fail closed");
        assert_eq!(error.code, CoreErrorCode::PermissionDenied);
    }

    #[test]
    fn connection_targets_reject_credential_like_host_and_base_path() {
        let registry = AdapterRegistry::new();
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        let reflected_secret = "sk-reflected-fixture-not-a-real-key";

        let origin = format!("https://{reflected_secret}.example.com");
        let connection = connection_for(&template, &origin);
        let error = registry
            .build_provider(&template, &connection)
            .err()
            .expect("credential-like host label must fail");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(!error.message.contains(reflected_secret));

        let mut connection = connection_for(&template, "https://example.com");
        let encoded_secret_path = "/sk%252Dreflected-fixture-not-a-real-key";
        connection.config.api_base_path =
            Some(EndpointPath::parse(encoded_secret_path).expect("encoded secret-like path"));
        let error = registry
            .build_provider(&template, &connection)
            .err()
            .expect("credential-like API base path must fail");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(!error.message.contains(encoded_secret_path));
    }

    #[test]
    fn adapter_registry_rejects_an_unimplemented_decoder_mode() {
        let registry = AdapterRegistry::new();
        let mut template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        template.default_manifest.decoders.streaming = None;
        let connection = connection_for(&template, "http://127.0.0.1:9");

        let error = registry
            .build_provider(&template, &connection)
            .err()
            .expect("unimplemented decoder mode must fail closed");
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
    }

    #[tokio::test]
    async fn custom_manifest_generation_uses_exact_endpoint_and_header_auth() {
        let (origin, requests, server) = fixture_server(vec![FixtureResponse {
            status: "200 OK",
            headers: vec![("Content-Type", "text/event-stream")],
            body: concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            )
            .to_owned(),
        }]);
        let template = custom_openai_template();
        let connection = connection_for(&template, &origin);
        let provider = AdapterRegistry::new()
            .build_provider(&template, &connection)
            .expect("custom generation provider");
        let (sink, _events) = tokio_mpsc::channel(4);
        let (_cancel_sender, cancelled) = watch::channel(false);

        provider
            .generate(
                generation_request("fixture-model"),
                Some(SYNTHETIC_CREDENTIAL),
                sink,
                cancelled,
            )
            .await
            .expect("custom generation");

        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("custom generation request");
        let lower = request.to_ascii_lowercase();
        assert!(request.starts_with("POST /tenant/generate HTTP/1.1\r\n"));
        assert!(lower.contains("x-fixture-key: synthetic-test-credential\r\n"));
        assert!(!lower.contains("authorization:"));
        server.join().expect("custom generation fixture server");
    }

    #[tokio::test]
    async fn custom_manifest_model_listing_uses_exact_endpoint_and_header_auth() {
        let (origin, requests, server) =
            fixture_server(vec![json_response(r#"{"data":[{"id":"fixture-model"}]}"#)]);
        let template = custom_openai_template();
        let connection = connection_for(&template, &origin);
        let listing = AdapterRegistry::new()
            .build_model_listing(&template, &connection)
            .expect("custom model listing");
        assert_eq!(listing.support(), ModelListSupport::Supported);
        let (_cancel_sender, cancelled) = watch::channel(false);

        let result = listing
            .list_models(ModelListRequest::new(Some(SYNTHETIC_CREDENTIAL), cancelled))
            .await
            .expect("custom model listing");
        assert_eq!(result.models[0].model_id, "fixture-model");
        assert_eq!(result.provenance.endpoint_path.as_str(), "/tenant/catalog");

        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("custom model-list request");
        let lower = request.to_ascii_lowercase();
        assert!(request.starts_with("GET /tenant/catalog HTTP/1.1\r\n"));
        assert!(lower.contains("x-fixture-key: synthetic-test-credential\r\n"));
        assert!(!lower.contains("authorization:"));
        server.join().expect("custom model-list fixture server");
    }

    #[tokio::test]
    async fn model_listing_rejects_a_provider_reflected_credential() {
        let reflected_secret = "sk-reflected-fixture-not-a-real-key";
        let body = format!(r#"{{"data":[{{"id":"{reflected_secret}"}}]}}"#);
        let (origin, requests, server) = fixture_server(vec![json_response(&body)]);
        let template = custom_openai_template();
        let connection = connection_for(&template, &origin);
        let listing = AdapterRegistry::new()
            .build_model_listing(&template, &connection)
            .expect("custom model listing");
        let (_cancel_sender, cancelled) = watch::channel(false);

        let error = listing
            .list_models(ModelListRequest::new(Some(SYNTHETIC_CREDENTIAL), cancelled))
            .await
            .expect_err("reflected credential must not become a listed model");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert!(!error.message.contains(reflected_secret));
        requests
            .recv_timeout(Duration::from_secs(5))
            .expect("single model-list request");
        server.join().expect("reflected credential fixture server");
    }

    #[tokio::test]
    async fn missing_models_endpoint_is_typed_generation_only_not_an_empty_catalog() {
        let mut template = custom_openai_template();
        template.default_manifest.endpoints.models = None;
        let connection = connection_for(&template, "http://127.0.0.1:9");
        let registry = AdapterRegistry::new();

        registry
            .build_provider(&template, &connection)
            .expect("generation remains available");
        let listing = registry
            .build_model_listing(&template, &connection)
            .expect("generation-only listing handle");
        assert_eq!(listing.support(), ModelListSupport::GenerationOnly);
        let (_cancel_sender, cancelled) = watch::channel(false);
        let error = listing
            .list_models(ModelListRequest::new(Some(SYNTHETIC_CREDENTIAL), cancelled))
            .await
            .expect_err("no endpoint must not look like an empty catalog");
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
    }

    #[test]
    fn route_config_is_exact_and_unsupported_cloud_mappings_fail_closed() {
        let template = custom_openai_template();
        let connection = connection_for(&template, "http://127.0.0.1:9");
        let registry = AdapterRegistry::new();
        let mut route = route_for(&connection, "fixture-model");
        route.route_config.endpoint_path =
            Some(EndpointPath::parse("/route/generate").expect("route endpoint"));
        registry
            .build_provider_for_route_with_plan(&template, &connection, &route, None)
            .expect("exact endpoint override is supported");

        route.route_config.deployment_id = Some("guessed-deployment".to_owned());
        let error = registry
            .build_provider_for_route_with_plan(&template, &connection, &route, None)
            .err()
            .expect("deployment mapping is not implemented by this family");
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);

        route.route_config.deployment_id = None;
        let reflected_secret = "sk-reflected-fixture-not-a-real-key";
        route.model_id = reflected_secret.to_owned();
        let error = registry
            .build_provider_for_route_with_plan(&template, &connection, &route, None)
            .err()
            .expect("credential-like route model ID must fail");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(!error.message.contains(reflected_secret));

        route.model_id = "x".repeat(MAX_MODEL_ID_BYTES + 1);
        let error = registry
            .build_provider_for_route_with_plan(&template, &connection, &route, None)
            .err()
            .expect("oversized route model ID must fail");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);

        route.model_id = "fixture-model".to_owned();
        route.route_config.endpoint_path = Some(
            EndpointPath::parse(&format!("/{reflected_secret}"))
                .expect("syntactically valid secret-like route path"),
        );
        let error = registry
            .build_provider_for_route_with_plan(&template, &connection, &route, None)
            .err()
            .expect("credential-like route endpoint must fail");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(!error.message.contains(reflected_secret));
    }

    #[tokio::test]
    async fn route_endpoint_override_is_the_actual_generation_target() {
        let (origin, requests, server) = fixture_server(vec![FixtureResponse {
            status: "200 OK",
            headers: vec![("Content-Type", "text/event-stream")],
            body: concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            )
            .to_owned(),
        }]);
        let template = custom_openai_template();
        let connection = connection_for(&template, &origin);
        let mut route = route_for(&connection, "fixture-model");
        route.route_config.endpoint_path =
            Some(EndpointPath::parse("/route/generate").expect("route endpoint"));
        let provider = AdapterRegistry::new()
            .build_provider_for_route_with_plan(&template, &connection, &route, None)
            .expect("route-aware provider");
        let (sink, _events) = tokio_mpsc::channel(4);
        let (_cancel_sender, cancelled) = watch::channel(false);

        provider
            .generate(
                generation_request("fixture-model"),
                Some(SYNTHETIC_CREDENTIAL),
                sink,
                cancelled,
            )
            .await
            .expect("route-aware generation");
        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("route generation request");
        assert!(request.starts_with("POST /route/generate HTTP/1.1\r\n"));
        server.join().expect("route generation fixture server");
    }

    #[test]
    fn registry_preview_is_exact_scalar_free_and_network_free() {
        let template = custom_openai_template();
        let connection = connection_for(&template, "https://example.com");
        let mut route = route_for(&connection, "private-model-name");
        route.route_config.endpoint_path =
            Some(EndpointPath::parse("/route/generate").expect("route endpoint"));

        let preview = AdapterRegistry::new()
            .preview_provider_request(&template, &connection, &route, None)
            .expect("safe request preview");
        assert_eq!(preview.method(), HttpMethod::Post);
        assert_eq!(preview.origin().as_str(), "https://example.com");
        assert_eq!(preview.path().as_str(), "/route/generate");
        assert_eq!(
            preview
                .header_names()
                .iter()
                .map(HeaderName::as_str)
                .collect::<Vec<_>>(),
            vec!["content-type", "x-fixture-key"]
        );
        assert!(!preview_top_level_fields(&preview).contains(&"stream_options"));
        let encoded = serde_json::to_string(&preview).expect("preview JSON");
        assert!(!encoded.contains("private-model-name"));
        assert!(!encoded.contains(SYNTHETIC_CREDENTIAL));

        let openrouter =
            AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter).expect("OpenRouter");
        let connection = connection_for(&openrouter, "https://openrouter.ai");
        let route = route_for(&connection, "openai/gpt-fixture");
        let preview = AdapterRegistry::new()
            .preview_provider_request(&openrouter, &connection, &route, None)
            .expect("OpenRouter preview");
        assert!(preview_top_level_fields(&preview).contains(&"stream_options"));
    }

    #[test]
    fn registry_preview_applies_the_compiled_gemini_route_expansion() {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::GeminiGenerateContent)
            .expect("Gemini template");
        let connection = connection_for(&template, "https://generativelanguage.googleapis.com");
        let mut route = route_for(&connection, "gemini-fixture");
        route.api_family = ApiFamily::GeminiGenerateContent;

        let preview = AdapterRegistry::new()
            .preview_provider_request(&template, &connection, &route, None)
            .expect("Gemini request preview");
        assert_eq!(
            preview.path().as_str(),
            "/v1beta/models/gemini-fixture:streamGenerateContent"
        );
        assert!(
            preview
                .header_names()
                .iter()
                .any(|name| name.as_str() == "x-goog-api-key")
        );
        let fields = preview_top_level_fields(&preview);
        assert!(fields.contains(&"systemInstruction"));
        assert!(fields.contains(&"generationConfig"));
    }

    #[test]
    fn registry_preview_exposes_stable_openai_responses_structure() {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI Responses template");
        let connection = connection_for(&template, "https://api.openai.com");
        let mut route = route_for(&connection, "gpt-fixture");
        route.api_family = ApiFamily::OpenAiResponses;
        let plan = ProviderRequestPlan {
            family: ApiFamily::OpenAiResponses,
            body_patches: Vec::new(),
            prompt_cache: PromptCacheDirective::None,
            cache_dialect: crate::parameter_mapping::PromptCacheWireDialect::Unsupported,
            preserve_opaque_reasoning_state: true,
        };

        let error = AdapterRegistry::new()
            .preview_provider_request(&template, &connection, &route, Some(&plan))
            .expect_err("Responses opaque continuity is unsupported");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);

        let preview = AdapterRegistry::new()
            .preview_provider_request(&template, &connection, &route, None)
            .expect("OpenAI Responses request preview");
        let fields = preview_top_level_fields(&preview);
        assert!(fields.contains(&"store"));
        assert!(!fields.contains(&"include"));
    }

    #[test]
    fn approved_lan_reconstructs_the_exact_persisted_policy() {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OllamaNative)
            .expect("Ollama template");
        let mut connection = connection_for(&template, "http://192.168.10.20:11434");
        let address = "192.168.10.20".parse().expect("fixture LAN address");
        connection.config.network_mode = ProviderNetworkMode::ApprovedLocalNetwork;
        connection.config.local_network_approval = Some(ProviderLocalNetworkApproval {
            origin: connection.api_origin.clone(),
            addresses: vec![address],
        });
        let registry = AdapterRegistry::new();
        registry
            .build_provider(&template, &connection)
            .expect("generation reconstructs the persisted LAN approval");
        registry
            .build_model_listing(&template, &connection)
            .expect("model listing reconstructs the persisted LAN approval");
        let mut route = route_for(&connection, "fixture-model");
        route.api_family = template.api_family;
        registry
            .preview_provider_request(&template, &connection, &route, None)
            .expect("preview reconstructs the persisted LAN approval");

        let approval = ApprovedLocalNetworkOrigin::new(connection.api_origin.as_str(), &[address])
            .expect("exact LAN approval");
        let policy = UrlPolicy::approved_local_network(approval);
        let engine = ParameterEngine::from_manifest_specs_for_family(
            ApiFamily::OllamaNative,
            &template.default_manifest.parameters,
        )
        .expect("built-in Ollama parameter contract");
        let parameters = engine
            .validate_for_request(&[ParameterValue {
                parameter_id: ParameterId::from("top_p"),
                state: ParameterValueState::Explicit(ParameterLiteral::Number(0.8)),
            }])
            .expect("valid Ollama preset");
        let plan = build_provider_request_plan(
            ApiFamily::OllamaNative,
            &parameters,
            &ReasoningSettings {
                mode: ReasoningMode::Enabled,
                effort: Some(ReasoningEffort::Low),
                preserve_opaque_state: false,
                ..ReasoningSettings::default()
            },
            &ReasoningWireDialect::OllamaLevel {
                efforts: vec![ReasoningEffort::Low],
                supports_disabled: false,
            },
            &PromptCacheSettings::default(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("valid exact Ollama request plan");
        registry
            .build_provider_with_plan_and_network_policy(
                &template,
                &connection,
                Some(plan.clone()),
                &policy,
            )
            .expect("request plan preserves the exact-origin LAN approval");
        let planned_preview = registry
            .preview_provider_request_with_network_policy(
                &template,
                &connection,
                &route,
                Some(&plan),
                &policy,
            )
            .expect("request-plan preview preserves the exact-origin LAN approval");
        let fields = preview_top_level_fields(&planned_preview);
        assert!(fields.contains(&"options"));
        assert!(fields.contains(&"think"));

        let wrong_approval = ApprovedLocalNetworkOrigin::new(
            "http://192.168.10.21:11434",
            &["192.168.10.21".parse().expect("wrong fixture LAN address")],
        )
        .expect("other exact LAN approval");
        let wrong_policy = UrlPolicy::approved_local_network(wrong_approval);
        let error = registry
            .build_provider_with_network_policy(&template, &connection, &wrong_policy)
            .err()
            .expect("another approved LAN origin must not be reused");
        assert_eq!(error.code, CoreErrorCode::PermissionDenied);
    }

    #[test]
    fn approved_lan_constructs_openai_wire_adapters_without_policy_downgrade() {
        let registry = AdapterRegistry::new();
        let origin = "http://192.168.10.20:8080";
        let address = "192.168.10.20".parse().expect("fixture LAN address");

        for template_id in [
            BuiltInTemplateId::OpenAiResponses,
            BuiltInTemplateId::OpenAiChatCompatible,
            BuiltInTemplateId::OpenRouter,
        ] {
            let template =
                AdapterRegistry::built_in_template(template_id).expect("built-in template");
            let mut connection = connection_for(&template, origin);
            connection.config.network_mode = ProviderNetworkMode::ApprovedLocalNetwork;
            connection.config.local_network_approval = Some(ProviderLocalNetworkApproval {
                origin: connection.api_origin.clone(),
                addresses: vec![address],
            });

            registry
                .build_provider(&template, &connection)
                .expect("exact approved LAN policy must reach the compiled adapter");
        }
    }

    #[test]
    fn every_built_in_contract_constructs_its_compiled_adapters() {
        let registry = AdapterRegistry::new();
        for id in BuiltInTemplateId::ALL {
            let template = AdapterRegistry::built_in_template(id).expect("built-in template");
            let origin = template
                .default_manifest
                .default_api_origin
                .as_ref()
                .map_or("https://api.example.com", CanonicalOrigin::as_str);
            let connection = connection_for(&template, origin);
            registry
                .build_provider(&template, &connection)
                .expect("compiled generation adapter");
            registry
                .build_model_listing(&template, &connection)
                .expect("compiled model-list adapter");
        }
    }

    #[tokio::test]
    async fn openai_model_listing_normalizes_and_sorts_records() {
        let (origin, requests, server) = fixture_server(vec![json_response(
            r#"{"object":"list","data":[{"id":"gpt-z"},{"id":"gpt-a"}]}"#,
        )]);
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        let connection = connection_for(&template, &origin);
        let listing = AdapterRegistry::new()
            .build_model_listing(&template, &connection)
            .expect("model listing");
        let (_cancel_sender, cancelled) = watch::channel(false);

        let result = listing
            .list_models(ModelListRequest::new(Some(SYNTHETIC_CREDENTIAL), cancelled))
            .await
            .expect("list OpenAI models");

        assert_eq!(
            result
                .models
                .iter()
                .map(|model| model.model_id.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-a", "gpt-z"]
        );
        assert!(
            result
                .models
                .iter()
                .all(|model| model.source == ModelRecordSource::ProviderApi)
        );
        assert_eq!(result.provenance.endpoint_path.as_str(), "/v1/models");
        assert_eq!(result.pages_fetched, 1);
        assert!(result.response_bytes > 0);

        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("recorded OpenAI request");
        assert!(request.starts_with("GET /v1/models HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-test-credential\r\n")
        );
        server.join().expect("OpenAI fixture server");
    }

    #[test]
    fn openrouter_metadata_discards_future_fields_and_retains_only_closed_capabilities() {
        let page = parse_openrouter_models(
            br#"{
                "data": [{
                    "id": "openai/closed-metadata",
                    "name": "Closed metadata",
                    "future_model_field": {"ignored": true},
                    "supported_parameters": [
                        "reasoning",
                        "include_reasoning",
                        "tools",
                        "parallel_tool_calls",
                        "structured_outputs",
                        "response_format",
                        "logprobs",
                        "seed",
                        "future_parameter"
                    ],
                    "reasoning": {
                        "supported_efforts": ["high", "future-effort-v9", "low"],
                        "default_effort": "high",
                        "default_enabled": true,
                        "supports_max_tokens": true,
                        "mandatory": false,
                        "future_reasoning_field": {"ignored": true}
                    }
                }]
            }"#,
        )
        .expect("normalize additive OpenRouter metadata");
        let capabilities = &page.models[0].capabilities;
        assert_eq!(
            capabilities.parameters,
            OpenRouterSupportedParameterSupport::Exact(vec![
                OpenRouterSupportedParameter::IncludeReasoning,
                OpenRouterSupportedParameter::Logprobs,
                OpenRouterSupportedParameter::ParallelToolCalls,
                OpenRouterSupportedParameter::Reasoning,
                OpenRouterSupportedParameter::ResponseFormat,
                OpenRouterSupportedParameter::Seed,
                OpenRouterSupportedParameter::StructuredOutputs,
                OpenRouterSupportedParameter::Tools,
            ])
        );
        assert_eq!(
            capabilities.supported,
            vec![
                super::ListedModelCapability::Reasoning,
                super::ListedModelCapability::ToolCalling,
                super::ListedModelCapability::ParallelToolCalling,
                super::ListedModelCapability::StructuredOutput,
                super::ListedModelCapability::JsonMode,
                super::ListedModelCapability::Logprobs,
                super::ListedModelCapability::Seed,
            ]
        );
        let reasoning = capabilities
            .reasoning
            .as_ref()
            .expect("closed reasoning metadata");
        assert_eq!(
            reasoning.supported_efforts,
            OpenRouterReasoningEffortSupport::Exact(vec![
                OpenRouterReasoningEffort::High,
                OpenRouterReasoningEffort::Low,
            ])
        );
        assert_eq!(
            reasoning.default_effort,
            Some(OpenRouterReasoningEffort::High)
        );
        let serialized = serde_json::to_string(capabilities).expect("serialize capabilities");
        assert!(!serialized.contains("future"));
    }

    #[test]
    fn openrouter_requires_non_null_supported_parameters() {
        for body in [
            r#"{"data":[{"id":"missing-parameters"}]}"#,
            r#"{"data":[{"id":"null-parameters","supported_parameters":null}]}"#,
            r#"{"data":[{"id":"wrong-parameters","supported_parameters":"reasoning"}]}"#,
        ] {
            let error = parse_openrouter_models(body.as_bytes())
                .expect_err("required supported_parameters must fail closed");
            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        }
    }

    #[test]
    fn openrouter_include_reasoning_never_invents_reasoning_support() {
        let page = parse_openrouter_models(
            br#"{"data":[{
                "id":"include-only",
                "supported_parameters":["include_reasoning"]
            }]}"#,
        )
        .expect("include-only metadata is a valid unsupported route");
        assert!(page.models[0].capabilities.supported.is_empty());
        assert_eq!(
            page.models[0].capabilities.parameters,
            OpenRouterSupportedParameterSupport::Exact(vec![
                OpenRouterSupportedParameter::IncludeReasoning,
            ])
        );

        let error = parse_openrouter_models(
            br#"{"data":[{
                "id":"contradictory-reasoning",
                "supported_parameters":["include_reasoning"],
                "reasoning":{"supported_efforts":["high"]}
            }]}"#,
        )
        .expect_err("structured reasoning requires an actionable request parameter");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
    }

    #[test]
    fn openrouter_legacy_efforts_distinguish_omitted_null_and_unknown_only() {
        let page = parse_openrouter_models(
            br#"{"data":[
                {
                    "id":"legacy-omitted",
                    "supported_parameters":["reasoning_effort"],
                    "reasoning":{}
                },
                {
                    "id":"legacy-null",
                    "supported_parameters":["reasoning_effort"],
                    "reasoning":{"supported_efforts":null}
                },
                {
                    "id":"legacy-future-only",
                    "supported_parameters":["reasoning_effort"],
                    "reasoning":{"supported_efforts":["future-effort-v9"]}
                }
            ]}"#,
        )
        .expect("normalize legacy reasoning effort metadata");
        assert_eq!(
            page.models[0]
                .capabilities
                .reasoning
                .as_ref()
                .expect("omitted reasoning")
                .supported_efforts,
            OpenRouterReasoningEffortSupport::NotExposed
        );
        assert_eq!(
            page.models[1]
                .capabilities
                .reasoning
                .as_ref()
                .expect("null reasoning")
                .supported_efforts,
            OpenRouterReasoningEffortSupport::AllGateway
        );
        assert_eq!(
            page.models[2]
                .capabilities
                .reasoning
                .as_ref()
                .expect("future-only reasoning")
                .supported_efforts,
            OpenRouterReasoningEffortSupport::Exact(Vec::new())
        );

        for body in [
            r#"{"data":[{"id":"null-known","supported_parameters":["reasoning"],"reasoning":{"default_enabled":null}}]}"#,
            r#"{"data":[{"id":"wrong-known","supported_parameters":["reasoning"],"reasoning":{"mandatory":"false"}}]}"#,
        ] {
            assert!(
                parse_openrouter_models(body.as_bytes()).is_err(),
                "known fields with null or wrong types must fail closed"
            );
        }
    }

    #[test]
    fn openrouter_metadata_canonicalizes_closed_sets_and_discards_unknown_defaults() {
        let page = parse_openrouter_models(
            br#"{"data":[
                {
                    "id":"canonical-dual",
                    "supported_parameters":[
                        "temperature","reasoning_effort","reasoning","temperature","reasoning"
                    ],
                    "reasoning":{
                        "supported_efforts":["none","high","future-effort-v9","high","none"],
                        "default_effort":"high"
                    }
                },
                {
                    "id":"unknown-default",
                    "supported_parameters":["reasoning_effort"],
                    "reasoning":{
                        "supported_efforts":["high"],
                        "default_effort":"future-effort-v9"
                    }
                },
                {
                    "id":"exact-none",
                    "supported_parameters":["reasoning_effort"],
                    "reasoning":{
                        "supported_efforts":["none"],
                        "default_effort":"none",
                        "default_enabled":false
                    }
                }
            ]}"#,
        )
        .expect("normalize duplicate and additive OpenRouter metadata");
        assert_eq!(
            page.models[0].capabilities.parameters,
            OpenRouterSupportedParameterSupport::Exact(vec![
                OpenRouterSupportedParameter::Reasoning,
                OpenRouterSupportedParameter::ReasoningEffort,
                OpenRouterSupportedParameter::Temperature,
            ])
        );
        assert_eq!(
            page.models[0]
                .capabilities
                .reasoning
                .as_ref()
                .expect("dual reasoning metadata")
                .supported_efforts,
            OpenRouterReasoningEffortSupport::Exact(vec![
                OpenRouterReasoningEffort::High,
                OpenRouterReasoningEffort::None,
            ])
        );
        let unknown_default = page.models[1]
            .capabilities
            .reasoning
            .as_ref()
            .expect("unknown default metadata");
        assert_eq!(unknown_default.default_effort, None);
        assert!(
            !serde_json::to_string(unknown_default)
                .expect("serialize normalized reasoning")
                .contains("future-effort-v9")
        );
        let exact_none = page.models[2]
            .capabilities
            .reasoning
            .as_ref()
            .expect("exact none metadata");
        assert_eq!(
            exact_none.supported_efforts,
            OpenRouterReasoningEffortSupport::Exact(vec![OpenRouterReasoningEffort::None,])
        );
        assert_eq!(
            exact_none.default_effort,
            Some(OpenRouterReasoningEffort::None)
        );
    }

    #[test]
    fn openrouter_metadata_rejects_contradictory_or_malformed_known_reasoning_fields() {
        for body in [
            r#"{"data":[{"id":"outside-exact","supported_parameters":["reasoning"],"reasoning":{"supported_efforts":["high","future-effort-v9"],"default_effort":"low"}}]}"#,
            r#"{"data":[{"id":"null-default","supported_parameters":["reasoning"],"reasoning":{"default_effort":null}}]}"#,
            r#"{"data":[{"id":"wrong-default","supported_parameters":["reasoning"],"reasoning":{"default_effort":false}}]}"#,
            r#"{"data":[{"id":"null-max","supported_parameters":["reasoning"],"reasoning":{"supports_max_tokens":null}}]}"#,
            r#"{"data":[{"id":"wrong-max","supported_parameters":["reasoning"],"reasoning":{"supports_max_tokens":"true"}}]}"#,
            r#"{"data":[{"id":"null-mandatory","supported_parameters":["reasoning"],"reasoning":{"mandatory":null}}]}"#,
            r#"{"data":[{"id":"wrong-mandatory","supported_parameters":["reasoning"],"reasoning":{"mandatory":"false"}}]}"#,
        ] {
            let error = parse_openrouter_models(body.as_bytes())
                .expect_err("contradictory, null, or wrong-typed known metadata must fail closed");
            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        }
    }

    #[test]
    fn openrouter_metadata_bounds_parameter_and_reasoning_arrays_before_filtering() {
        let invalid_parameters = [
            vec!["temperature".to_owned(); super::MAX_SUPPORTED_PARAMETER_COUNT + 1],
            vec!["x".repeat(super::MAX_SUPPORTED_PARAMETER_BYTES + 1)],
            vec!["temperature\n".to_owned()],
            vec!["sk-short-private-value".to_owned()],
        ];
        for supported_parameters in invalid_parameters {
            let body = serde_json::to_vec(&serde_json::json!({
                "data": [{
                    "id": "invalid-parameters",
                    "supported_parameters": supported_parameters,
                }],
            }))
            .expect("serialize invalid parameter fixture");
            let error = parse_openrouter_models(&body)
                .expect_err("parameter bounds apply before closed-set filtering");
            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        }

        let invalid_efforts = [
            vec!["high".to_owned(); super::MAX_REASONING_EFFORT_COUNT + 1],
            vec!["x".repeat(super::MAX_REASONING_EFFORT_BYTES + 1)],
            vec!["high\n".to_owned()],
            vec!["sk-short-private-value".to_owned()],
        ];
        for supported_efforts in invalid_efforts {
            let body = serde_json::to_vec(&serde_json::json!({
                "data": [{
                    "id": "invalid-efforts",
                    "supported_parameters": ["reasoning"],
                    "reasoning": {"supported_efforts": supported_efforts},
                }],
            }))
            .expect("serialize invalid effort fixture");
            let error = parse_openrouter_models(&body)
                .expect_err("reasoning bounds apply before closed-set filtering");
            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        }
    }

    #[test]
    fn openrouter_rejects_mandatory_reasoning_contradictions() {
        for body in [
            r#"{"data":[{"id":"mandatory-none","supported_parameters":["reasoning"],"reasoning":{"supported_efforts":["high","none"],"mandatory":true}}]}"#,
            r#"{"data":[{"id":"mandatory-disabled-default","supported_parameters":["reasoning"],"reasoning":{"supported_efforts":["high"],"default_enabled":false,"mandatory":true}}]}"#,
        ] {
            let error = parse_openrouter_models(body.as_bytes())
                .expect_err("mandatory reasoning metadata cannot expose a disabled state");
            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        }
    }

    #[tokio::test]
    async fn openrouter_metadata_parser_requires_the_exact_builtin_template_version() {
        let body = r#"{"data":[{"id":"openai/version-gated"}]}"#;
        let (origin, _requests, server) = fixture_server(vec![json_response(body)]);
        let exact = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("exact OpenRouter template");
        let exact_connection = connection_for(&exact, &origin);
        let exact_listing = AdapterRegistry::new()
            .build_model_listing(&exact, &exact_connection)
            .expect("exact OpenRouter listing");
        let (_cancel_sender, cancelled) = watch::channel(false);
        let error = exact_listing
            .list_models(ModelListRequest::new(Some(SYNTHETIC_CREDENTIAL), cancelled))
            .await
            .expect_err("exact OpenRouter requires supported_parameters");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        server.join().expect("exact OpenRouter fixture");

        let (origin, _requests, server) = fixture_server(vec![json_response(body)]);
        let mut stale = exact;
        stale.manifest_version = stale.manifest_version.saturating_sub(1);
        let stale_connection = connection_for(&stale, &origin);
        let stale_listing = AdapterRegistry::new()
            .build_model_listing(&stale, &stale_connection)
            .expect("stale generic listing");
        let (_cancel_sender, cancelled) = watch::channel(false);
        let result = stale_listing
            .list_models(ModelListRequest::new(Some(SYNTHETIC_CREDENTIAL), cancelled))
            .await
            .expect("wrong-version template uses only the family-default parser");
        assert_eq!(result.models[0].model_id, "openai/version-gated");
        assert_eq!(
            result.models[0].capabilities.parameters,
            OpenRouterSupportedParameterSupport::NotExposed
        );
        server.join().expect("stale OpenRouter fixture");
    }

    #[tokio::test]
    async fn openrouter_model_listing_uses_its_compiled_api_v1_base_path() {
        let (origin, requests, server) = fixture_server(vec![json_response(
            r#"{"data":[{"id":"openai/gpt-fixture","name":"Fixture","context_length":131072,"top_provider":{"max_completion_tokens":16384},"supported_parameters":["temperature","top_p","max_tokens"]}]}"#,
        )]);
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("OpenRouter template");
        let connection = connection_for(&template, &origin);
        let listing = AdapterRegistry::new()
            .build_model_listing(&template, &connection)
            .expect("OpenRouter model listing");
        let (_cancel_sender, cancelled) = watch::channel(false);

        let result = listing
            .list_models(ModelListRequest::new(Some(SYNTHETIC_CREDENTIAL), cancelled))
            .await
            .expect("list OpenRouter models");

        assert_eq!(result.models[0].model_id, "openai/gpt-fixture");
        assert_eq!(result.models[0].display_name.as_deref(), Some("Fixture"));
        assert_eq!(result.models[0].max_input_tokens, Some(131_072));
        assert_eq!(result.models[0].max_output_tokens, Some(16_384));
        assert_eq!(
            result.provenance.api_family,
            lorepia_domain::ApiFamily::OpenAiChatCompletions
        );
        assert_eq!(result.provenance.endpoint_path.as_str(), "/api/v1/models");

        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("recorded OpenRouter request");
        assert!(request.starts_with("GET /api/v1/models HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-test-credential\r\n")
        );
        server.join().expect("OpenRouter fixture server");
    }

    #[tokio::test]
    async fn openrouter_generation_adapter_posts_to_its_exact_chat_endpoint() {
        let opaque_canary = "registry-openrouter-opaque-canary";
        let (origin, requests, server) = fixture_server(vec![FixtureResponse {
            status: "200 OK",
            headers: vec![("Content-Type", "text/event-stream")],
            body: format!(
                "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"reasoning_details\":[{{\"type\":\"reasoning.encrypted\",\"data\":\"{opaque_canary}\",\"id\":\"detail-1\",\"format\":\"anthropic-claude-v1\",\"index\":0}}]}}}}]}}\n\n\
                 data: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\n\
                 data: [DONE]\n\n"
            ),
        }]);
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("OpenRouter template");
        let connection = connection_for(&template, &origin);
        let parameters =
            ParameterEngine::from_manifest_specs(&template.default_manifest.parameters)
                .expect("OpenRouter parameters")
                .validate_for_request(&[])
                .expect("provider defaults");
        let plan = build_provider_request_plan(
            ApiFamily::OpenAiChatCompletions,
            &parameters,
            &ReasoningSettings {
                mode: ReasoningMode::Enabled,
                effort: Some(ReasoningEffort::High),
                preserve_opaque_state: false,
                ..ReasoningSettings::default()
            },
            &ReasoningWireDialect::OpenRouter {
                style: OpenRouterReasoningWireStyle::Unified,
                supported_efforts: OpenRouterReasoningEffortSupport::Exact(vec![
                    OpenRouterReasoningEffort::High,
                ]),
                default_effort: None,
                default_enabled: None,
                supports_max_tokens: None,
                mandatory: Some(false),
            },
            &PromptCacheSettings::default(),
            PromptCacheWireDialect::Unsupported,
        )
        .expect("generic chat reasoning plan");
        let provider = AdapterRegistry::new()
            .build_provider_with_plan(&template, &connection, Some(plan))
            .expect("OpenRouter generation adapter");
        let (sink, mut events) = tokio_mpsc::channel(4);
        let (_cancel_sender, cancelled) = watch::channel(false);

        provider
            .generate(
                GenerationRequest {
                    generation_id: GenerationId::new(),
                    conversation_id: ConversationId::new(),
                    model: "openai/gpt-fixture".to_owned(),
                    messages: Vec::new(),
                    resolved_prompt_plan: None,
                    provider_execution_plan_hash: None,
                    temperature: None,
                    max_output_tokens: None,
                    provider_provenance: Some(GenerationProviderProvenance {
                        api_family: ApiFamily::OpenAiChatCompletions,
                        model_route_id: ModelRouteId::from("openrouter-route"),
                        generation_preset_id: GenerationPresetId::from("openrouter-preset"),
                    }),
                    preserve_opaque_reasoning_state: false,
                    opaque_reasoning_context: Vec::new(),
                },
                Some(SYNTHETIC_CREDENTIAL),
                sink,
                cancelled,
            )
            .await
            .expect("OpenRouter generation");
        assert!(events.try_recv().is_err());

        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("recorded OpenRouter generation request");
        assert!(request.starts_with("POST /api/v1/chat/completions HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-test-credential\r\n")
        );
        let (_, body) = request
            .split_once("\r\n\r\n")
            .expect("OpenRouter request body");
        let body: serde_json::Value = serde_json::from_str(body).expect("OpenRouter body JSON");
        assert_eq!(body["reasoning"]["effort"], "high");
        assert!(body.get("reasoning_effort").is_none());
        server.join().expect("OpenRouter generation fixture server");
    }

    #[tokio::test]
    async fn anthropic_model_listing_follows_bounded_cursor_pagination() {
        let (origin, requests, server) = fixture_server(vec![
            json_response(
                r#"{"data":[{"id":"claude-a","display_name":"Claude A","max_input_tokens":200000,"max_tokens":8192}],"has_more":true,"last_id":"claude-a"}"#,
            ),
            json_response(
                r#"{"data":[{"id":"claude-b","display_name":"Claude B","max_input_tokens":100000,"max_tokens":4096}],"has_more":false,"last_id":"claude-b"}"#,
            ),
        ]);
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::AnthropicMessages)
            .expect("Anthropic template");
        let connection = connection_for(&template, &origin);
        let listing = AdapterRegistry::new()
            .build_model_listing(&template, &connection)
            .expect("model listing");
        let (_cancel_sender, cancelled) = watch::channel(false);

        let result = listing
            .list_models(ModelListRequest::new(Some(SYNTHETIC_CREDENTIAL), cancelled))
            .await
            .expect("list Anthropic models");
        assert_eq!(result.models.len(), 2);
        assert_eq!(result.models[0].max_input_tokens, Some(200_000));
        assert_eq!(result.models[0].max_output_tokens, Some(8_192));
        assert_eq!(result.pages_fetched, 2);

        let first = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("first Anthropic request");
        let second = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("second Anthropic request");
        assert!(first.starts_with("GET /v1/models?limit=1000 HTTP/1.1\r\n"));
        assert!(second.starts_with("GET /v1/models?limit=1000&after_id=claude-a HTTP/1.1\r\n"));
        for request in [first, second] {
            let request = request.to_ascii_lowercase();
            assert!(request.contains("x-api-key: synthetic-test-credential\r\n"));
            assert!(request.contains("anthropic-version: 2023-06-01\r\n"));
        }
        server.join().expect("Anthropic fixture server");
    }

    #[tokio::test]
    async fn gemini_listing_keeps_generate_and_embedding_models() {
        let (origin, requests, server) = fixture_server(vec![json_response(
            r#"{"models":[
                {"name":"models/gemini-chat","displayName":"Gemini Chat","inputTokenLimit":1000,"outputTokenLimit":100,"supportedGenerationMethods":["generateContent"]},
                {"name":"models/text-embedding","displayName":"Embedding","supportedGenerationMethods":["embedContent"]}
            ]}"#,
        )]);
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::GeminiGenerateContent)
            .expect("Gemini template");
        let connection = connection_for(&template, &origin);
        let listing = AdapterRegistry::new()
            .build_model_listing(&template, &connection)
            .expect("model listing");
        let (_cancel_sender, cancelled) = watch::channel(false);

        let result = listing
            .list_models(ModelListRequest::new(Some(SYNTHETIC_CREDENTIAL), cancelled))
            .await
            .expect("list Gemini models");
        assert_eq!(result.models.len(), 2);
        assert_eq!(result.models[0].model_id, "gemini-chat");
        assert_eq!(
            result.models[0].supported_generation_methods,
            vec!["generateContent"]
        );
        assert_eq!(result.models[1].model_id, "text-embedding");
        assert_eq!(
            result.models[1].supported_generation_methods,
            vec!["embedContent"]
        );

        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("recorded Gemini request");
        assert!(request.starts_with("GET /v1beta/models?pageSize=1000 HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("x-goog-api-key: synthetic-test-credential\r\n")
        );
        server.join().expect("Gemini fixture server");
    }

    #[tokio::test]
    async fn ollama_listing_is_credential_free_and_uses_native_tags() {
        let (origin, requests, server) = fixture_server(vec![json_response(
            r#"{"models":[{"name":"gemma3:latest","model":"gemma3:latest"}]}"#,
        )]);
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OllamaNative)
            .expect("Ollama template");
        let localhost_origin = origin.replacen("127.0.0.1", "localhost", 1);
        let connection = connection_for(&template, &localhost_origin);
        let listing = AdapterRegistry::new()
            .build_model_listing(&template, &connection)
            .expect("model listing");
        let (_cancel_sender, cancelled) = watch::channel(false);

        let result = listing
            .list_models(ModelListRequest::new(None, cancelled))
            .await
            .expect("list Ollama models");
        assert_eq!(result.models[0].model_id, "gemma3:latest");
        assert_eq!(
            result.models[0].display_name.as_deref(),
            Some("gemma3:latest")
        );

        let request = requests
            .recv_timeout(Duration::from_secs(5))
            .expect("recorded Ollama request");
        assert!(request.starts_with("GET /api/tags HTTP/1.1\r\n"));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        server.join().expect("Ollama fixture server");
    }

    #[tokio::test]
    async fn model_listing_requires_exact_credential_scope() {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        let mut connection = connection_for(&template, "http://127.0.0.1:9");
        connection.credential_scope = None;

        let error = AdapterRegistry::new()
            .build_model_listing(&template, &connection)
            .err()
            .expect("missing approval must fail");
        assert_eq!(error.code, CoreErrorCode::PermissionDenied);
    }

    #[test]
    fn connection_network_mode_must_match_its_origin_scope() {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        let mut connection = connection_for(&template, "http://127.0.0.1:9");
        connection.config.network_mode = ProviderNetworkMode::Public;
        let public_error = AdapterRegistry::new()
            .build_model_listing(&template, &connection)
            .err()
            .expect("public mode must reject loopback");
        assert_eq!(public_error.code, CoreErrorCode::InvalidInput);

        connection.api_origin =
            canonical_origin("https://api.example.test").expect("remote fixture origin");
        connection.config.network_mode = ProviderNetworkMode::LocalLoopback;
        connection
            .credential_scope
            .as_mut()
            .expect("credential scope")
            .allowed_origins = vec![connection.api_origin.clone()];
        let local_error = AdapterRegistry::new()
            .build_model_listing(&template, &connection)
            .err()
            .expect("local mode must reject public hosts");
        assert_eq!(local_error.code, CoreErrorCode::InvalidInput);
    }

    #[tokio::test]
    async fn model_listing_does_not_follow_redirects() {
        let (origin, requests, server) = fixture_server(vec![FixtureResponse {
            status: "302 Found",
            headers: vec![("Location", "https://example.invalid/models")],
            body: String::new(),
        }]);
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        let connection = connection_for(&template, &origin);
        let listing = AdapterRegistry::new()
            .build_model_listing(&template, &connection)
            .expect("model listing");
        let (_cancel_sender, cancelled) = watch::channel(false);

        let error = listing
            .list_models(ModelListRequest::new(Some(SYNTHETIC_CREDENTIAL), cancelled))
            .await
            .expect_err("redirect must not be followed");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        requests
            .recv_timeout(Duration::from_secs(5))
            .expect("single redirect request");
        assert!(requests.try_recv().is_err());
        server.join().expect("redirect fixture server");
    }

    #[tokio::test]
    async fn cancellation_is_observed_before_network_io() {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiResponses)
            .expect("OpenAI template");
        let connection = connection_for(&template, "http://127.0.0.1:9");
        let listing = AdapterRegistry::new()
            .build_model_listing(&template, &connection)
            .expect("model listing");
        let (_cancel_sender, cancelled) = watch::channel(true);

        let error = listing
            .list_models(ModelListRequest::new(Some(SYNTHETIC_CREDENTIAL), cancelled))
            .await
            .expect_err("cancelled request");
        assert_eq!(error.code, CoreErrorCode::Cancelled);
    }

    #[test]
    fn model_pages_reject_duplicate_and_invalid_records_before_persistence() {
        let page = parse_model_page(
            lorepia_domain::ApiFamily::OpenAiResponses,
            br#"{"data":[{"id":"same"},{"id":"same"}]}"#,
        )
        .expect("page parser permits cross-page reconciliation to detect duplicates");
        assert_eq!(page.models.len(), 2);

        let invalid = parse_model_page(
            lorepia_domain::ApiFamily::OpenAiResponses,
            b"{\"data\":[{\"id\":\"bad\\nmodel\"}]}",
        )
        .expect_err("control character must be rejected");
        assert_eq!(invalid.code, CoreErrorCode::ProviderUnavailable);
    }

    #[test]
    fn model_pages_reject_reflected_credentials_before_persistence() {
        let reflected_secret = "sk-reflected-fixture-not-a-real-key";

        for (family, body) in [
            (
                ApiFamily::OpenAiResponses,
                format!(r#"{{"data":[{{"id":"{reflected_secret}"}}]}}"#),
            ),
            (
                ApiFamily::AnthropicMessages,
                format!(
                    r#"{{"data":[{{"id":"claude-safe","display_name":"Model {reflected_secret}"}}],"has_more":false}}"#
                ),
            ),
            (
                ApiFamily::GeminiGenerateContent,
                format!(
                    r#"{{"models":[{{"name":"models/gemini-safe","supportedGenerationMethods":["generateContent","{reflected_secret}"]}}]}}"#
                ),
            ),
        ] {
            let error = parse_model_page(family, body.as_bytes())
                .expect_err("credential-like catalog field must be rejected");
            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
            assert!(!error.message.contains(reflected_secret));
        }
    }

    #[test]
    fn model_list_budget_can_only_tighten_hard_limits() {
        assert!(ModelListBudget::new(1, 1, 1, 1).is_ok());
        assert!(ModelListBudget::new(0, 1, 1, 1).is_err());
        assert!(ModelListBudget::new(33, 1, 1, 1).is_err());
        assert!(ModelListBudget::new(1, 10_001, 1, 1).is_err());
        assert!(ModelListBudget::new(1, 1, 2, 1).is_err());
    }
}
