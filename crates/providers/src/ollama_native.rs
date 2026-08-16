use std::{collections::BTreeSet, net::IpAddr, time::Duration};

use async_trait::async_trait;
use futures_util::StreamExt;
use lorepia_domain::{
    ApiFamily, AuthBinding, CoreError, CoreErrorCode, CoreResult, GenerationRequest,
    GenerationUsage, MessageRole, ProviderCapabilities, ToolCallArgumentsDelta, ToolCallId,
    ToolName,
};
use reqwest::{
    RequestBuilder, Response, StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue},
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use url::{Host, Url};

use crate::{
    Provider, ProviderEvent, ProviderEventSender, merge_usage_summary,
    network_transport::{ProviderHttpTarget, authorize_request, validate_credential_for_auth},
    parameter_mapping::{PromptCacheWireDialect, ProviderRequestPlan},
    prompt_contract::ProviderPromptAdapterContract,
    request_plan::planned_json_payload,
    url_policy::{CanonicalUrl, UrlPolicyMode},
};

const MAX_JSONL_LINE_BYTES: usize = 1024 * 1024;
const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
const MAX_MODEL_CATALOG_BYTES: usize = 4 * 1024 * 1024;
const MAX_MODEL_COUNT: usize = 10_000;
const MAX_OLLAMA_TOOL_CALLS: u32 = 128;
const MAX_PROMPT_MESSAGES: usize = 128;
const MAX_PROMPT_BYTES: usize = 512 * 1024;
const MAX_PROMPT_CHARS: usize = 128 * 1024;
const MAX_MODEL_ID_BYTES: usize = 1024;
const MAX_MODEL_ID_CHARS: usize = 256;
const MAX_MODEL_METADATA_BYTES: usize = 4096;
const MAX_MODEL_METADATA_CHARS: usize = 1024;
const MAX_MODEL_FAMILIES: usize = 64;

/// A model reported by Ollama's native `GET /api/tags` endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaModelSummary {
    pub model_id: String,
    pub display_name: String,
    pub modified_at: Option<String>,
    pub size: Option<u64>,
    pub digest: Option<String>,
    pub details: Option<OllamaModelDetails>,
}

/// Portable model metadata returned by Ollama.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaModelDetails {
    pub format: Option<String>,
    pub family: Option<String>,
    pub families: Vec<String>,
    pub parameter_size: Option<String>,
    pub quantization_level: Option<String>,
}

/// Native Ollama `/api/chat` and `/api/tags` adapter.
///
/// [`Self::new`] is deliberately unauthenticated. Call
/// [`Self::new_with_approved_bearer`] only after the caller has approved the
/// exact HTTPS origin that may receive a bearer credential.
#[derive(Clone)]
pub struct OllamaNativeProvider {
    chat_endpoint: Url,
    tags_endpoint: Option<Url>,
    chat_target: ProviderHttpTarget,
    tags_target: Option<ProviderHttpTarget>,
    bearer_auth_approved: bool,
    manifest_auth: Option<AuthBinding>,
    request_plan: Option<ProviderRequestPlan>,
}

impl OllamaNativeProvider {
    /// Creates an unauthenticated provider for HTTPS or loopback HTTP.
    pub fn new(base_url: &str, timeout: Duration) -> CoreResult<Self> {
        Self::build(base_url, timeout, false)
    }

    /// Creates a provider allowed to send a bearer credential to this HTTPS
    /// origin.
    ///
    /// Calling this constructor is the caller's explicit origin approval.
    /// Redirects remain disabled, so the credential cannot be forwarded to a
    /// redirect target.
    pub fn new_with_approved_bearer(base_url: &str, timeout: Duration) -> CoreResult<Self> {
        Self::build(base_url, timeout, true)
    }

    fn build(base_url: &str, timeout: Duration, bearer_auth_approved: bool) -> CoreResult<Self> {
        if timeout.is_zero() {
            return Err(CoreError::invalid(
                "Ollama request timeout must be greater than zero",
            ));
        }

        let parsed =
            Url::parse(base_url).map_err(|_| CoreError::invalid("invalid Ollama base URL"))?;
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(CoreError::invalid(
                "Ollama base URL must not contain a query or fragment",
            ));
        }
        let policy_mode = if parsed.host().is_some_and(is_loopback_host) {
            UrlPolicyMode::LocalLoopback
        } else {
            UrlPolicyMode::Public
        };
        let base = CanonicalUrl::parse(base_url, policy_mode)
            .map_err(|_| CoreError::invalid("Ollama base URL is not allowed"))?
            .into_url();
        if bearer_auth_approved && base.scheme() != "https" {
            return Err(CoreError::invalid(
                "Ollama bearer authentication requires an approved HTTPS origin",
            ));
        }

        let chat_endpoint = api_endpoint(&base, "chat");
        let tags_endpoint = api_endpoint(&base, "tags");
        let chat_target = ProviderHttpTarget::inferred(chat_endpoint.as_str(), timeout)?;
        let tags_target = ProviderHttpTarget::inferred(tags_endpoint.as_str(), timeout)?;

        Ok(Self {
            chat_endpoint,
            tags_endpoint: Some(tags_endpoint),
            chat_target,
            tags_target: Some(tags_target),
            bearer_auth_approved,
            manifest_auth: None,
            request_plan: None,
        })
    }

    pub(crate) fn new_with_manifest_targets(
        chat_target: ProviderHttpTarget,
        tags_target: Option<ProviderHttpTarget>,
        auth: AuthBinding,
    ) -> Self {
        Self {
            chat_endpoint: chat_target.url().clone(),
            tags_endpoint: tags_target.as_ref().map(|target| target.url().clone()),
            chat_target,
            tags_target,
            bearer_auth_approved: false,
            manifest_auth: Some(auth),
            request_plan: None,
        }
    }

    #[must_use]
    pub fn with_request_plan(mut self, plan: ProviderRequestPlan) -> Self {
        self.request_plan = Some(plan);
        self
    }

    pub(crate) fn with_optional_request_plan(mut self, plan: Option<ProviderRequestPlan>) -> Self {
        self.request_plan = plan;
        self
    }

    /// Lists models visible through Ollama's native `GET /api/tags` endpoint.
    pub async fn list_models(
        &self,
        credential: Option<&str>,
        mut cancelled: watch::Receiver<bool>,
    ) -> CoreResult<Vec<OllamaModelSummary>> {
        ensure_not_cancelled(&cancelled)?;
        if let Some(auth) = &self.manifest_auth {
            validate_credential_for_auth(auth, credential)?;
        }
        let target = self.tags_target.as_ref().ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::UnsupportedContent,
                "provider manifest does not declare a model-list endpoint",
                false,
            )
        })?;
        let endpoint = self
            .tags_endpoint
            .as_ref()
            .ok_or_else(|| CoreError::internal("model-list target is missing its endpoint"))?;
        let prepared = target.prepare().await?;
        ensure_not_cancelled(&cancelled)?;
        let request = self.authorize(prepared.client().get(endpoint.clone()), credential)?;
        let mut cancellation_open = true;
        let response =
            send_with_cancellation(request, &mut cancelled, &mut cancellation_open).await?;
        prepared.validate_response_peer(&response)?;
        ensure_success(response.status())?;
        ensure_json_content_type(response.headers())?;
        if response
            .content_length()
            .is_some_and(|size| size > MAX_MODEL_CATALOG_BYTES as u64)
        {
            return Err(catalog_error("Ollama model catalog exceeded 4 MiB"));
        }

        let body = collect_limited_body(
            response,
            MAX_MODEL_CATALOG_BYTES,
            &mut cancelled,
            &mut cancellation_open,
        )
        .await?;
        let value: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|_| catalog_error("Ollama returned a malformed model catalog"))?;
        let object = value
            .as_object()
            .ok_or_else(|| catalog_error("Ollama returned a malformed model catalog"))?;
        if object.get("error").is_some_and(|error| !error.is_null()) {
            return Err(catalog_error("Ollama returned a model catalog error"));
        }
        let envelope: ModelListResponse = serde_json::from_value(value)
            .map_err(|_| catalog_error("Ollama returned a malformed model catalog"))?;
        normalize_models(envelope.models)
    }

    fn authorize(
        &self,
        request: RequestBuilder,
        credential: Option<&str>,
    ) -> CoreResult<RequestBuilder> {
        if let Some(auth) = &self.manifest_auth {
            return authorize_request(request, auth, credential);
        }
        let Some(credential) = credential.filter(|value| !value.is_empty()) else {
            return Ok(request);
        };
        if !self.bearer_auth_approved || self.chat_endpoint.scheme() != "https" {
            return Err(CoreError::invalid(
                "Ollama bearer credential is not approved for this origin",
            ));
        }
        let mut value = HeaderValue::from_str(&format!("Bearer {credential}"))
            .map_err(|_| CoreError::invalid("invalid Ollama bearer credential"))?;
        value.set_sensitive(true);
        Ok(request.header(AUTHORIZATION, value))
    }
}

#[async_trait]
impl Provider for OllamaNativeProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: true,
            max_context_tokens: None,
        }
    }

    fn snapshot_request(&self, request: &GenerationRequest) -> CoreResult<serde_json::Value> {
        crate::validate_snapshot_request(request)?;
        validate_request(request)?;
        let cache_dialect = self
            .request_plan
            .as_ref()
            .map_or(PromptCacheWireDialect::Unsupported, |plan| {
                plan.cache_dialect()
            });
        let payload = chat_request(request.clone(), cache_dialect)?;
        planned_json_payload(
            &payload,
            ApiFamily::OllamaNative,
            self.request_plan.as_ref(),
        )
    }

    async fn generate(
        &self,
        request: GenerationRequest,
        credential: Option<&str>,
        sink: ProviderEventSender,
        mut cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        ensure_not_cancelled(&cancelled)?;
        validate_request(&request)?;

        let cache_dialect = self
            .request_plan
            .as_ref()
            .map_or(PromptCacheWireDialect::Unsupported, |plan| {
                plan.cache_dialect()
            });
        let payload = chat_request(request, cache_dialect)?;
        let payload = planned_json_payload(
            &payload,
            ApiFamily::OllamaNative,
            self.request_plan.as_ref(),
        )?;
        if let Some(auth) = &self.manifest_auth {
            validate_credential_for_auth(auth, credential)?;
        }
        let prepared = self.chat_target.prepare().await?;
        ensure_not_cancelled(&cancelled)?;
        let request = self.authorize(
            prepared
                .client()
                .post(self.chat_endpoint.clone())
                .json(&payload),
            credential,
        )?;
        let mut cancellation_open = true;
        let response =
            send_with_cancellation(request, &mut cancelled, &mut cancellation_open).await?;
        prepared.validate_response_peer(&response)?;
        ensure_success(response.status())?;
        ensure_ndjson_content_type(response.headers())?;
        if response
            .content_length()
            .is_some_and(|size| size > MAX_STREAM_BYTES as u64)
        {
            return Err(stream_too_large_error());
        }

        consume_chat_stream(response, &sink, &mut cancelled, &mut cancellation_open).await
    }

    async fn generate_with_internal_plan(
        &self,
        request: GenerationRequest,
        credential: Option<&str>,
        sink: ProviderEventSender,
        cancelled: watch::Receiver<bool>,
        request_plan: ProviderRequestPlan,
    ) -> CoreResult<GenerationUsage> {
        self.clone()
            .with_optional_request_plan(Some(request_plan))
            .generate(request, credential, sink, cancelled)
            .await
    }
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatRequestMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<ChatOptions>,
}

#[derive(Serialize)]
struct ChatRequestMessage {
    role: &'static str,
    content: String,
}

#[derive(Serialize)]
struct ChatOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}

fn chat_request(
    request: GenerationRequest,
    cache_dialect: PromptCacheWireDialect,
) -> CoreResult<ChatRequest> {
    let messages = if let Some(plan) = &request.resolved_prompt_plan {
        let compiled = prompt_contract()
            .compile_resolved_plan_for_execution(plan, cache_dialect)
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        compiled
            .verify_execution_hash(request.provider_execution_plan_hash.as_deref())
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        compiled
            .messages()
            .iter()
            .map(|message| ChatRequestMessage {
                role: message.wire_role().as_str(),
                content: message.content().to_owned(),
            })
            .collect()
    } else {
        request
            .messages
            .into_iter()
            .map(|message| ChatRequestMessage {
                role: role_name(message.role),
                content: message.content,
            })
            .collect()
    };
    Ok(ChatRequest {
        model: request.model,
        messages,
        stream: true,
        options: (request.temperature.is_some() || request.max_output_tokens.is_some()).then_some(
            ChatOptions {
                temperature: request.temperature,
                num_predict: request.max_output_tokens,
            },
        ),
    })
}

fn validate_request(request: &GenerationRequest) -> CoreResult<()> {
    if request.resolved_prompt_plan.is_none() && request.provider_execution_plan_hash.is_some() {
        return Err(CoreError::invalid(
            "provider execution plan hash requires a resolved prompt plan",
        ));
    }
    if request.model.trim().is_empty()
        || request.model.trim() != request.model
        || request.model.len() > MAX_MODEL_ID_BYTES
        || request.model.chars().count() > MAX_MODEL_ID_CHARS
        || request.model.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(
            "Ollama model is not a bounded identifier",
        ));
    }
    let prompt_message_count = request
        .resolved_prompt_plan
        .as_ref()
        .map_or(request.messages.len(), |plan| plan.effective_messages.len());
    if prompt_message_count > MAX_PROMPT_MESSAGES {
        return Err(CoreError::invalid(
            "Ollama prompt exceeded the message-count safety limit",
        ));
    }
    if prompt_message_count == 0 {
        return Err(CoreError::invalid(
            "Ollama chat requests require at least one message",
        ));
    }
    let mut total_bytes = 0_usize;
    let mut total_chars = 0_usize;
    if let Some(plan) = &request.resolved_prompt_plan {
        for message in &plan.effective_messages {
            total_bytes = total_bytes
                .checked_add(message.content.len())
                .ok_or_else(prompt_too_large_error)?;
            total_chars = total_chars
                .checked_add(message.content.chars().count())
                .ok_or_else(prompt_too_large_error)?;
            if total_bytes > MAX_PROMPT_BYTES || total_chars > MAX_PROMPT_CHARS {
                return Err(prompt_too_large_error());
            }
        }
    } else {
        for message in &request.messages {
            if message.conversation_id != request.conversation_id {
                return Err(CoreError::invalid(
                    "Ollama prompt contains a message from another conversation",
                ));
            }
            total_bytes = total_bytes
                .checked_add(message.content.len())
                .ok_or_else(prompt_too_large_error)?;
            total_chars = total_chars
                .checked_add(message.content.chars().count())
                .ok_or_else(prompt_too_large_error)?;
            if total_bytes > MAX_PROMPT_BYTES || total_chars > MAX_PROMPT_CHARS {
                return Err(prompt_too_large_error());
            }
        }
    }
    if request
        .temperature
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(CoreError::invalid(
            "Ollama temperature must be a non-negative finite number",
        ));
    }
    if request.max_output_tokens == Some(0) {
        return Err(CoreError::invalid(
            "Ollama max output tokens must be greater than zero",
        ));
    }
    if request.preserve_opaque_reasoning_state || !request.opaque_reasoning_context.is_empty() {
        return Err(CoreError::invalid(
            "Ollama does not support opaque reasoning continuity",
        ));
    }
    Ok(())
}

fn prompt_too_large_error() -> CoreError {
    CoreError::invalid("Ollama prompt exceeded the input safety limit")
}

#[derive(Deserialize)]
struct ChatChunk {
    message: Option<ChatResponseMessage>,
    done: Option<bool>,
    total_duration: Option<u64>,
    load_duration: Option<u64>,
    prompt_eval_count: Option<u64>,
    prompt_eval_duration: Option<u64>,
    eval_count: Option<u64>,
    eval_duration: Option<u64>,
}

#[derive(Default, Deserialize)]
struct ChatResponseMessage {
    role: Option<String>,
    #[serde(default)]
    content: String,
    #[serde(default)]
    thinking: String,
    #[serde(default)]
    tool_calls: Vec<OllamaToolCall>,
}

#[derive(Deserialize)]
struct OllamaToolCall {
    id: Option<String>,
    function: OllamaFunctionCall,
}

#[derive(Deserialize)]
struct OllamaFunctionCall {
    name: Option<String>,
    arguments: Option<serde_json::Value>,
}

#[derive(Default)]
struct ChatStreamState {
    saw_payload: bool,
    next_tool_call_index: u32,
    tool_call_ids: BTreeSet<ToolCallId>,
    usage: GenerationUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineAction {
    Continue,
    Done,
}

async fn consume_chat_stream(
    response: Response,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<GenerationUsage> {
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut state = ChatStreamState::default();
    let mut total_stream_bytes = 0_usize;

    loop {
        tokio::select! {
            biased;
            change = cancelled.changed(), if *cancellation_open => {
                if change.is_err() {
                    *cancellation_open = false;
                } else {
                    ensure_not_cancelled(cancelled)?;
                }
            }
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };
                let chunk = chunk.map_err(network_error)?;
                total_stream_bytes = total_stream_bytes
                    .checked_add(chunk.len())
                    .ok_or_else(stream_too_large_error)?;
                ensure_stream_size(total_stream_bytes)?;
                for segment in chunk.split_inclusive(|byte| *byte == b'\n') {
                    let terminated = segment.last() == Some(&b'\n');
                    let content = if terminated {
                        &segment[..segment.len() - 1]
                    } else {
                        segment
                    };
                    let combined_ends_with_carriage_return = content
                        .last()
                        .copied()
                        .or_else(|| pending.last().copied())
                        == Some(b'\r');
                    let trailing_carriage_return =
                        usize::from(terminated && combined_ends_with_carriage_return);
                    let projected_size = pending
                        .len()
                        .saturating_add(content.len())
                        .saturating_sub(trailing_carriage_return);
                    ensure_jsonl_line_size(projected_size)?;
                    pending.extend_from_slice(content);
                    if !terminated {
                        continue;
                    }
                    if pending.last() == Some(&b'\r') {
                        pending.pop();
                    }
                    if process_chat_line(
                        &pending,
                        sink,
                        &mut state,
                        cancelled,
                        cancellation_open,
                    )
                    .await?
                        == LineAction::Done
                    {
                        ensure_not_cancelled(cancelled)?;
                        return Ok(state.usage);
                    }
                    pending.clear();
                }
                ensure_pending_jsonl_size(&pending)?;
            }
        }
    }

    if pending.last() == Some(&b'\r') {
        pending.pop();
    }
    ensure_jsonl_line_size(pending.len())?;
    if !pending.is_empty()
        && process_chat_line(&pending, sink, &mut state, cancelled, cancellation_open).await?
            == LineAction::Done
    {
        ensure_not_cancelled(cancelled)?;
        return Ok(state.usage);
    }

    if state.saw_payload {
        Err(streaming_error(
            "Ollama stream ended before its terminal response",
        ))
    } else {
        Err(streaming_error(
            "Ollama returned an empty streaming response",
        ))
    }
}

async fn process_chat_line(
    line: &[u8],
    sink: &ProviderEventSender,
    state: &mut ChatStreamState,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<LineAction> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(LineAction::Continue);
    }

    let value: serde_json::Value = serde_json::from_slice(line)
        .map_err(|_| streaming_error("Ollama returned malformed streaming data"))?;
    let object = value
        .as_object()
        .ok_or_else(|| streaming_error("Ollama returned malformed streaming data"))?;
    if object.get("error").is_some_and(|error| !error.is_null()) {
        return Err(streaming_error("Ollama returned a streaming error"));
    }

    let chunk: ChatChunk = serde_json::from_value(value)
        .map_err(|_| streaming_error("Ollama returned malformed streaming data"))?;
    let done = chunk
        .done
        .ok_or_else(|| streaming_error("Ollama returned malformed streaming data"))?;
    let message = chunk
        .message
        .ok_or_else(|| streaming_error("Ollama returned malformed streaming data"))?;

    state.saw_payload = true;
    if let Some(count) = chunk.prompt_eval_count {
        state.usage.input_tokens = Some(count);
    }
    if let Some(count) = chunk.eval_count {
        state.usage.output_tokens = Some(count);
    }
    state.usage.provider_raw_summary = merge_usage_summary(
        state.usage.provider_raw_summary.as_ref(),
        &[
            ("total_duration", chunk.total_duration),
            ("load_duration", chunk.load_duration),
            ("prompt_eval_duration", chunk.prompt_eval_duration),
            ("eval_duration", chunk.eval_duration),
        ],
    );

    if message.role.as_deref() != Some("assistant") {
        return Err(streaming_error(
            "Ollama returned a non-assistant chat message",
        ));
    }
    if !message.thinking.is_empty() {
        send_provider_event(
            sink,
            ProviderEvent::ReasoningDelta(message.thinking),
            cancelled,
            cancellation_open,
        )
        .await?;
    }
    if !message.content.is_empty() {
        send_provider_event(
            sink,
            ProviderEvent::TextDelta(message.content),
            cancelled,
            cancellation_open,
        )
        .await?;
    }
    for tool_call in message.tool_calls {
        emit_ollama_tool_call(tool_call, state, sink, cancelled, cancellation_open).await?;
    }

    Ok(if done {
        LineAction::Done
    } else {
        LineAction::Continue
    })
}

async fn emit_ollama_tool_call(
    tool_call: OllamaToolCall,
    state: &mut ChatStreamState,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<()> {
    let index = state.next_tool_call_index;
    state.next_tool_call_index = state
        .next_tool_call_index
        .checked_add(1)
        .filter(|count| *count <= MAX_OLLAMA_TOOL_CALLS)
        .ok_or_else(|| streaming_error("Ollama returned too many tool calls"))?;
    let id = match tool_call.id {
        Some(id) => ToolCallId::parse(id),
        None => ToolCallId::parse(format!("ollama-call-{index}")),
    }
    .map_err(|_| streaming_error("Ollama returned an invalid tool-call id"))?;
    if !state.tool_call_ids.insert(id.clone()) {
        return Err(streaming_error("Ollama reused a tool-call identifier"));
    }
    let name = tool_call
        .function
        .name
        .ok_or_else(|| streaming_error("Ollama returned a tool call without a name"))
        .and_then(|name| {
            ToolName::parse(name)
                .map_err(|_| streaming_error("Ollama returned an invalid tool name"))
        })?;
    let arguments = tool_call
        .function
        .arguments
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    if !arguments.is_object() {
        return Err(streaming_error(
            "Ollama returned non-object tool-call arguments",
        ));
    }
    let arguments = serde_json::to_string(&arguments)
        .map_err(|_| streaming_error("Ollama returned invalid tool-call arguments"))
        .and_then(|arguments| {
            ToolCallArgumentsDelta::parse(arguments).map_err(|_| {
                streaming_error("Ollama tool-call arguments exceeded its safety limit")
            })
        })?;

    for event in [
        ProviderEvent::ToolCallStarted {
            id: id.clone(),
            name,
        },
        ProviderEvent::ToolCallArgumentsDelta {
            id: id.clone(),
            delta: arguments,
        },
        ProviderEvent::ToolCallCompleted { id },
    ] {
        send_provider_event(sink, event, cancelled, cancellation_open).await?;
    }
    Ok(())
}

async fn send_provider_event(
    sink: &ProviderEventSender,
    event: ProviderEvent,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<()> {
    ensure_not_cancelled(cancelled)?;
    let send = sink.send(event);
    tokio::pin!(send);
    loop {
        tokio::select! {
            biased;
            change = cancelled.changed(), if *cancellation_open => {
                if change.is_err() {
                    *cancellation_open = false;
                } else {
                    ensure_not_cancelled(cancelled)?;
                }
            }
            result = &mut send => {
                return result
                    .map_err(|_| CoreError::internal("provider event receiver closed"));
            }
        }
    }
}

async fn send_with_cancellation(
    request: RequestBuilder,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<Response> {
    let response = request.send();
    tokio::pin!(response);
    loop {
        tokio::select! {
            biased;
            change = cancelled.changed(), if *cancellation_open => {
                if change.is_err() {
                    *cancellation_open = false;
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
    limit: usize,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<Vec<u8>> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    loop {
        tokio::select! {
            biased;
            change = cancelled.changed(), if *cancellation_open => {
                if change.is_err() {
                    *cancellation_open = false;
                } else {
                    ensure_not_cancelled(cancelled)?;
                }
            }
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };
                let chunk = chunk.map_err(network_error)?;
                if body.len().saturating_add(chunk.len()) > limit {
                    return Err(catalog_error("Ollama model catalog exceeded 4 MiB"));
                }
                body.extend_from_slice(&chunk);
            }
        }
    }
    ensure_not_cancelled(cancelled)?;
    Ok(body)
}

#[derive(Deserialize)]
struct ModelListResponse {
    models: Vec<RawModelSummary>,
}

#[derive(Deserialize)]
struct RawModelSummary {
    name: Option<String>,
    model: Option<String>,
    modified_at: Option<String>,
    size: Option<u64>,
    digest: Option<String>,
    details: Option<RawModelDetails>,
}

#[derive(Deserialize)]
struct RawModelDetails {
    format: Option<String>,
    family: Option<String>,
    #[serde(default)]
    families: Vec<String>,
    parameter_size: Option<String>,
    quantization_level: Option<String>,
}

fn normalize_models(models: Vec<RawModelSummary>) -> CoreResult<Vec<OllamaModelSummary>> {
    if models.len() > MAX_MODEL_COUNT {
        return Err(catalog_error(
            "Ollama model catalog contained too many models",
        ));
    }
    models
        .into_iter()
        .map(|model| {
            let name = normalize_optional_model_identifier(model.name)?;
            let model_id = normalize_optional_model_identifier(model.model)?;
            let model_id = model_id
                .or_else(|| name.clone())
                .ok_or_else(|| catalog_error("Ollama model catalog contained an invalid model"))?;
            let display_name = name.unwrap_or_else(|| model_id.clone());
            let modified_at = normalize_optional_model_metadata(model.modified_at)?;
            let digest = normalize_optional_model_metadata(model.digest)?;
            let details = model.details.map(normalize_model_details).transpose()?;
            Ok(OllamaModelSummary {
                model_id,
                display_name,
                modified_at,
                size: model.size,
                digest,
                details,
            })
        })
        .collect()
}

fn normalize_optional_model_identifier(value: Option<String>) -> CoreResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    if value.trim() != value
        || value.len() > MAX_MODEL_ID_BYTES
        || value.chars().count() > MAX_MODEL_ID_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(catalog_error(
            "Ollama model catalog contained an invalid model",
        ));
    }
    Ok(Some(value))
}

fn normalize_optional_model_metadata(value: Option<String>) -> CoreResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if value.trim() != value
        || value.len() > MAX_MODEL_METADATA_BYTES
        || value.chars().count() > MAX_MODEL_METADATA_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(catalog_error(
            "Ollama model catalog contained invalid metadata",
        ));
    }
    Ok(Some(value))
}

fn normalize_model_details(details: RawModelDetails) -> CoreResult<OllamaModelDetails> {
    if details.families.len() > MAX_MODEL_FAMILIES {
        return Err(catalog_error(
            "Ollama model catalog contained invalid metadata",
        ));
    }
    let families = details
        .families
        .into_iter()
        .map(|family| {
            normalize_optional_model_metadata(Some(family))?
                .ok_or_else(|| catalog_error("Ollama model catalog contained invalid metadata"))
        })
        .collect::<CoreResult<Vec<_>>>()?;
    Ok(OllamaModelDetails {
        format: normalize_optional_model_metadata(details.format)?,
        family: normalize_optional_model_metadata(details.family)?,
        families,
        parameter_size: normalize_optional_model_metadata(details.parameter_size)?,
        quantization_level: normalize_optional_model_metadata(details.quantization_level)?,
    })
}

fn ensure_jsonl_line_size(size: usize) -> CoreResult<()> {
    if size > MAX_JSONL_LINE_BYTES {
        return Err(streaming_error("Ollama streaming line exceeded 1 MiB"));
    }
    Ok(())
}

fn ensure_pending_jsonl_size(pending: &[u8]) -> CoreResult<()> {
    let payload_size = pending
        .len()
        .saturating_sub(usize::from(pending.last() == Some(&b'\r')));
    ensure_jsonl_line_size(payload_size)
}

fn ensure_stream_size(size: usize) -> CoreResult<()> {
    if size > MAX_STREAM_BYTES {
        return Err(stream_too_large_error());
    }
    Ok(())
}

fn stream_too_large_error() -> CoreError {
    streaming_error("Ollama stream exceeded 64 MiB")
}

fn ensure_ndjson_content_type(headers: &reqwest::header::HeaderMap) -> CoreResult<()> {
    let is_ndjson = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/x-ndjson"));
    if is_ndjson {
        Ok(())
    } else {
        Err(streaming_error(
            "Ollama returned an unexpected streaming content type",
        ))
    }
}

fn ensure_json_content_type(headers: &reqwest::header::HeaderMap) -> CoreResult<()> {
    let is_json = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"));
    if is_json {
        Ok(())
    } else {
        Err(catalog_error(
            "Ollama returned an unexpected model catalog content type",
        ))
    }
}

fn is_loopback_host(host: Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let name = name.trim_end_matches('.');
            name.eq_ignore_ascii_case("localhost")
                || name
                    .to_ascii_lowercase()
                    .strip_suffix(".localhost")
                    .is_some_and(|prefix| !prefix.is_empty())
        }
        Host::Ipv4(address) => IpAddr::V4(address).is_loopback(),
        Host::Ipv6(address) => IpAddr::V6(address).is_loopback(),
    }
}

fn api_endpoint(base: &Url, resource: &str) -> Url {
    let mut endpoint = base.clone();
    let ends_with_api = endpoint
        .path_segments()
        .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
        == Some("api");
    {
        let mut segments = endpoint
            .path_segments_mut()
            .expect("validated HTTP URL supports path segments");
        segments.pop_if_empty();
        if !ends_with_api {
            segments.push("api");
        }
        segments.push(resource);
    }
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    endpoint
}

pub(crate) const fn prompt_contract() -> ProviderPromptAdapterContract {
    ProviderPromptAdapterContract::for_family(ApiFamily::OllamaNative)
}

#[cfg(test)]
pub(crate) fn resolved_prompt_payload_for_contract_test(
    request: GenerationRequest,
    cache_dialect: PromptCacheWireDialect,
) -> CoreResult<serde_json::Value> {
    let payload = chat_request(request, cache_dialect)?;
    serde_json::to_value(payload)
        .map_err(|_| CoreError::internal("failed to serialize Ollama test payload"))
}

fn role_name(role: MessageRole) -> &'static str {
    prompt_contract().map_message_role(role).wire_role.as_str()
}

fn ensure_not_cancelled(cancelled: &watch::Receiver<bool>) -> CoreResult<()> {
    if *cancelled.borrow() {
        Err(CoreError::new(
            CoreErrorCode::Cancelled,
            "generation was cancelled",
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
    let (code, recoverable) = match status {
        StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE => {
            (CoreErrorCode::InvalidInput, false)
        }
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            (CoreErrorCode::ProviderAuthFailed, false)
        }
        StatusCode::NOT_FOUND => (CoreErrorCode::NotFound, false),
        StatusCode::TOO_MANY_REQUESTS => (CoreErrorCode::ProviderRateLimited, true),
        _ => (
            CoreErrorCode::ProviderUnavailable,
            status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT,
        ),
    };
    Err(CoreError::new(
        code,
        format!("Ollama returned HTTP {}", status.as_u16()),
        recoverable,
    ))
}

fn network_error(error: reqwest::Error) -> CoreError {
    let (code, message) = if error.is_timeout() {
        (
            CoreErrorCode::ProviderUnavailable,
            "Ollama request timed out",
        )
    } else if error.is_connect() {
        (
            CoreErrorCode::NetworkUnavailable,
            "could not connect to Ollama",
        )
    } else {
        (CoreErrorCode::NetworkUnavailable, "Ollama request failed")
    };
    CoreError::new(code, message, true)
}

fn streaming_error(message: &'static str) -> CoreError {
    CoreError::new(CoreErrorCode::ProviderUnavailable, message, true)
}

fn catalog_error(message: &'static str) -> CoreError {
    CoreError::new(CoreErrorCode::ProviderUnavailable, message, true)
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        io::{self, Read, Write},
        net::{TcpListener, TcpStream},
        process::Command,
        sync::mpsc as std_mpsc,
        thread,
        time::Instant,
    };

    use lorepia_domain::{
        ConversationId, GenerationId, GenerationPresetId, GenerationRequest, Message, ModelRouteId,
        OpaqueReasoningContext, OpaqueReasoningData, OpaqueReasoningState,
    };
    use tokio::sync::{mpsc, watch};

    use super::*;
    use crate::parameter_mapping::{PromptCacheDirective, RequestBodyPatch};

    struct SyntheticResponse {
        status: &'static str,
        content_type: &'static str,
        body: Vec<u8>,
        fragment_bytes: usize,
        extra_headers: &'static str,
    }

    fn request() -> GenerationRequest {
        let conversation_id = ConversationId::new();
        GenerationRequest {
            generation_id: GenerationId::new(),
            conversation_id: conversation_id.clone(),
            model: "fixture-model".to_owned(),
            messages: vec![Message::user(conversation_id, "hello")],
            resolved_prompt_plan: None,
            provider_execution_plan_hash: None,
            temperature: Some(0.7),
            max_output_tokens: Some(128),
            provider_provenance: None,
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        }
    }

    fn read_http_request(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
        const MAX_REQUEST_BYTES: usize = 128 * 1024;
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let (headers_len, content_len) = loop {
            let read = stream.read(&mut buffer)?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "request ended before its headers",
                ));
            }
            request.extend_from_slice(&buffer[..read]);
            if request.len() > MAX_REQUEST_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "request exceeded test server limit",
                ));
            }
            let Some(headers_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
            else {
                continue;
            };
            let headers_len = headers_end + 4;
            let headers = std::str::from_utf8(&request[..headers_end])
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let content_len = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            break (headers_len, content_len);
        };

        while request.len() < headers_len + content_len {
            let read = stream.read(&mut buffer)?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "request ended before its body",
                ));
            }
            request.extend_from_slice(&buffer[..read]);
            if request.len() > MAX_REQUEST_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "request exceeded test server limit",
                ));
            }
        }
        Ok(request)
    }

    fn synthetic_server(response: SyntheticResponse) -> (String, std_mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind synthetic server");
        let address = listener.local_addr().expect("synthetic server address");
        let (request_sender, request_receiver) = std_mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let request = read_http_request(&mut stream).expect("read request");
            let _ = request_sender.send(request);
            let fragment_bytes = response.fragment_bytes.max(1);
            if write!(
                stream,
                "HTTP/1.1 {}\r\nContent-Type: {}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n{}\r\n",
                response.status, response.content_type, response.extra_headers
            )
            .is_err()
            {
                return;
            }
            for fragment in response.body.chunks(fragment_bytes) {
                if write!(stream, "{:X}\r\n", fragment.len()).is_err()
                    || stream.write_all(fragment).is_err()
                    || stream.write_all(b"\r\n").is_err()
                {
                    return;
                }
            }
            let _ = stream.write_all(b"0\r\n\r\n");
        });
        (format!("http://{address}"), request_receiver)
    }

    fn proxy_probe_server() -> (String, std_mpsc::Sender<()>, std_mpsc::Receiver<bool>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy probe");
        listener
            .set_nonblocking(true)
            .expect("nonblocking proxy probe");
        let address = listener.local_addr().expect("proxy probe address");
        let (stop_sender, stop_receiver) = std_mpsc::channel();
        let (observed_sender, observed_receiver) = std_mpsc::channel();
        thread::spawn(move || {
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                        let _ = observed_sender.send(true);
                        return;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(_) => {
                        let _ = observed_sender.send(true);
                        return;
                    }
                }
                if stop_receiver.try_recv().is_ok() {
                    let _ = observed_sender.send(false);
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        (format!("http://{address}"), stop_sender, observed_receiver)
    }

    fn stalling_catalog_server() -> (String, std_mpsc::Receiver<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stalling catalog server");
        let address = listener.local_addr().expect("stalling server address");
        let (started_sender, started_receiver) = std_mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            read_http_request(&mut stream).expect("read request");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nB\r\n{\"models\":[\r\n",
                )
                .expect("write partial catalog");
            stream.flush().expect("flush partial catalog");
            started_sender.send(()).expect("catalog started");
            thread::sleep(Duration::from_secs(1));
            let _ = stream.write_all(b"2\r\n]}\r\n0\r\n\r\n");
        });
        (format!("http://{address}"), started_receiver)
    }

    async fn generate_from_body(
        body: &[u8],
        fragment_bytes: usize,
    ) -> (CoreResult<GenerationUsage>, Vec<ProviderEvent>, Vec<u8>) {
        let (base_url, captured) = synthetic_server(SyntheticResponse {
            status: "200 OK",
            content_type: "application/x-ndjson",
            body: body.to_vec(),
            fragment_bytes,
            extra_headers: "",
        });
        let provider =
            OllamaNativeProvider::new(&base_url, Duration::from_secs(2)).expect("provider");
        let (sink, mut events) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let result = provider.generate(request(), None, sink, cancelled).await;
        let events = std::iter::from_fn(|| events.try_recv().ok()).collect();
        (result, events, captured.recv().expect("captured request"))
    }

    #[test]
    fn enforces_transport_and_authentication_policy() {
        assert!(
            OllamaNativeProvider::new("http://example.com:11434", Duration::from_secs(1)).is_err()
        );
        assert!(
            OllamaNativeProvider::new("https://user:secret@example.com", Duration::from_secs(1))
                .is_err()
        );
        assert!(
            OllamaNativeProvider::new(
                "https://example.com?credential=secret",
                Duration::from_secs(1)
            )
            .is_err()
        );
        assert!(
            OllamaNativeProvider::new("http://127.0.0.2:11434", Duration::from_secs(1)).is_ok()
        );
        assert!(OllamaNativeProvider::new("http://[::1]:11434", Duration::from_secs(1)).is_ok());
        let rooted_localhost =
            OllamaNativeProvider::new("http://localhost.:11434", Duration::from_secs(1))
                .expect("root-dot localhost uses the loopback policy");
        assert_eq!(rooted_localhost.chat_endpoint.host_str(), Some("localhost"));
        let rooted_subdomain =
            OllamaNativeProvider::new("http://model.localhost.:11434", Duration::from_secs(1))
                .expect("root-dot localhost subdomain uses the loopback policy");
        assert_eq!(
            rooted_subdomain.chat_endpoint.host_str(),
            Some("model.localhost")
        );
        assert!(
            OllamaNativeProvider::new_with_approved_bearer(
                "http://127.0.0.1:11434",
                Duration::from_secs(1)
            )
            .is_err()
        );
        assert!(
            OllamaNativeProvider::new_with_approved_bearer(
                "https://ollama.com/api",
                Duration::from_secs(1)
            )
            .is_ok()
        );
        for blocked in [
            "https://10.0.0.1",
            "https://169.254.169.254",
            "https://192.168.1.2",
            "https://[fe80::1]",
        ] {
            assert!(
                OllamaNativeProvider::new(blocked, Duration::from_secs(1)).is_err(),
                "blocked URL: {blocked}"
            );
        }
    }

    #[test]
    fn loopback_requests_bypass_environment_proxy() {
        const CHILD_FLAG: &str = "LOREPIA_OLLAMA_PROXY_CHILD";
        const DESTINATION_URL: &str = "LOREPIA_OLLAMA_PROXY_DESTINATION";
        if env::var_os(CHILD_FLAG).is_some() {
            let destination = env::var(DESTINATION_URL).expect("child destination URL");
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("child runtime");
            runtime.block_on(async move {
                let provider = OllamaNativeProvider::new(&destination, Duration::from_secs(2))
                    .expect("child provider");
                let (sink, _events) = mpsc::channel(1);
                let (_cancel, cancelled) = watch::channel(false);
                provider
                    .generate(request(), None, sink, cancelled)
                    .await
                    .expect("direct loopback request");
            });
            return;
        }

        let (destination, captured) = synthetic_server(SyntheticResponse {
            status: "200 OK",
            content_type: "application/x-ndjson",
            body: br#"{"message":{"role":"assistant","content":""},"done":true}
"#
            .to_vec(),
            fragment_bytes: 2,
            extra_headers: "",
        });
        let (proxy, stop_proxy, proxy_observed) = proxy_probe_server();
        let output = Command::new(env::current_exe().expect("current test executable"))
            .arg("loopback_requests_bypass_environment_proxy")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(CHILD_FLAG, "1")
            .env(DESTINATION_URL, &destination)
            .env("HTTP_PROXY", &proxy)
            .env("http_proxy", &proxy)
            .env("ALL_PROXY", &proxy)
            .env("all_proxy", &proxy)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .output()
            .expect("run isolated proxy test");
        stop_proxy.send(()).expect("stop proxy probe");

        assert!(
            output.status.success(),
            "proxy child failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        captured
            .recv_timeout(Duration::from_secs(1))
            .expect("destination received direct request");
        assert!(
            !proxy_observed
                .recv_timeout(Duration::from_secs(1))
                .expect("proxy observation"),
            "loopback request reached the configured proxy"
        );
    }

    #[test]
    fn approved_https_provider_builds_only_a_sensitive_bearer_header() {
        let provider = OllamaNativeProvider::new_with_approved_bearer(
            "https://ollama.com/api",
            Duration::from_secs(1),
        )
        .expect("approved HTTPS provider");
        let client = reqwest::Client::new();
        let tags_endpoint = provider
            .tags_endpoint
            .clone()
            .expect("direct provider has a tags endpoint");
        let request = provider
            .authorize(client.get(tags_endpoint.clone()), Some("synthetic-secret"))
            .expect("authorized request")
            .build()
            .expect("request");
        let authorization = request
            .headers()
            .get(AUTHORIZATION)
            .expect("authorization header");

        assert_eq!(
            authorization.to_str().expect("ASCII header"),
            "Bearer synthetic-secret"
        );
        assert!(authorization.is_sensitive());

        let error = provider
            .authorize(client.get(tags_endpoint), Some("invalid\nsecret"))
            .expect_err("control characters must be rejected");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(!error.message.contains("secret"));
    }

    #[tokio::test]
    async fn unauthenticated_provider_rejects_supplied_credentials() {
        let provider = OllamaNativeProvider::new("http://127.0.0.1:9", Duration::from_secs(1))
            .expect("provider");
        let (sink, _events) = mpsc::channel(1);
        let (_cancel, cancelled) = watch::channel(false);
        let error = provider
            .generate(request(), Some("synthetic-secret"), sink, cancelled)
            .await
            .expect_err("credential must be rejected before connecting");

        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(!error.message.contains("synthetic-secret"));
    }

    #[tokio::test]
    async fn streams_fragmented_jsonl_and_maps_usage() {
        let body = concat!(
            "{\"model\":\"fixture-model\",\"message\":{\"role\":\"assistant\",\"thinking\":\"생각\"},\"done\":false}\r\n",
            "{\"model\":\"fixture-model\",\"message\":{\"role\":\"assistant\",\"content\":\"안녕\"},\"done\":false}\n",
            "{\"model\":\"fixture-model\",\"message\":{\"role\":\"assistant\",\"content\":\"!\"},\"done\":true,\"done_reason\":\"stop\",\"total_duration\":100,\"load_duration\":10,\"prompt_eval_count\":7,\"prompt_eval_duration\":20,\"eval_count\":3,\"eval_duration\":30}\n",
            "{\"error\":\"must be ignored after terminal\"}\n",
        );

        let (result, events, request) = generate_from_body(body.as_bytes(), 2).await;

        let usage = result.expect("valid stream");
        assert_eq!(usage.input_tokens, Some(7));
        assert_eq!(usage.output_tokens, Some(3));
        assert_eq!(usage.cached_read_tokens, None);
        assert_eq!(usage.cached_write_tokens, None);
        assert_eq!(usage.reasoning_tokens, None);
        assert_eq!(usage.tool_tokens, None);
        assert_eq!(
            usage
                .provider_raw_summary
                .as_ref()
                .map(lorepia_domain::BoundedJson::as_str),
            Some(
                r#"{"eval_duration":30,"load_duration":10,"prompt_eval_duration":20,"total_duration":100}"#
            )
        );
        assert_eq!(
            events,
            vec![
                ProviderEvent::ReasoningDelta("생각".to_owned()),
                ProviderEvent::TextDelta("안녕".to_owned()),
                ProviderEvent::TextDelta("!".to_owned()),
            ]
        );
        let request = String::from_utf8(request).expect("UTF-8 request");
        assert!(request.starts_with("POST /api/chat HTTP/1.1\r\n"));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        let body = request.split_once("\r\n\r\n").expect("request body").1;
        let payload: serde_json::Value = serde_json::from_str(body).expect("request JSON");
        assert_eq!(payload["model"], "fixture-model");
        assert_eq!(payload["stream"], true);
        assert_eq!(payload["options"]["temperature"], 0.7);
        assert_eq!(payload["options"]["num_predict"], 128);
    }

    #[tokio::test]
    async fn request_plan_is_applied_to_the_exact_ollama_wire_payload() {
        let response = br#"{"message":{"role":"assistant","content":""},"done":true}"#;
        let (base_url, captured) = synthetic_server(SyntheticResponse {
            status: "200 OK",
            content_type: "application/x-ndjson",
            body: response.to_vec(),
            fragment_bytes: response.len(),
            extra_headers: "",
        });
        let plan = ProviderRequestPlan {
            family: ApiFamily::OllamaNative,
            body_patches: vec![
                RequestBodyPatch {
                    path: "think".to_owned(),
                    value: serde_json::json!("low"),
                },
                RequestBodyPatch {
                    path: "options.top_p".to_owned(),
                    value: serde_json::json!(0.8),
                },
            ],
            prompt_cache: PromptCacheDirective::None,
            cache_dialect: crate::parameter_mapping::PromptCacheWireDialect::Unsupported,
            preserve_opaque_reasoning_state: false,
        };
        let provider = OllamaNativeProvider::new(&base_url, Duration::from_secs(2))
            .expect("provider")
            .with_request_plan(plan);
        let (sink, _events) = mpsc::channel(4);
        let (_cancel, cancelled) = watch::channel(false);

        provider
            .generate(request(), None, sink, cancelled)
            .await
            .expect("planned request");

        let request =
            String::from_utf8(captured.recv().expect("captured request")).expect("UTF-8 request");
        let body = request.split_once("\r\n\r\n").expect("request body").1;
        let payload: serde_json::Value = serde_json::from_str(body).expect("request JSON");
        assert_eq!(payload["think"], "low");
        assert_eq!(payload["options"]["top_p"], 0.8);
        assert_eq!(payload["options"]["temperature"], 0.7);
        assert_eq!(payload["options"]["num_predict"], 128);
    }

    #[tokio::test]
    async fn accepts_terminal_json_without_a_trailing_newline() {
        let body =
            br#"{"message":{"role":"assistant","content":"complete"},"done":true,"eval_count":1}"#;
        let (result, events, _) = generate_from_body(body, 1).await;

        assert_eq!(
            result.expect("complete final JSON value"),
            GenerationUsage {
                input_tokens: None,
                output_tokens: Some(1),
                ..GenerationUsage::default()
            }
        );
        assert_eq!(
            events,
            vec![ProviderEvent::TextDelta("complete".to_owned())]
        );
    }

    #[tokio::test]
    async fn represents_tool_calls_as_inert_protocol_events() {
        let body = br#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"lookup","arguments":{"q":"seoul"}}}]},"done":true,"eval_count":1}
"#;
        let (result, events, _) = generate_from_body(body, body.len()).await;

        result.expect("Ollama tool call must be a normal protocol state");
        assert!(matches!(
            &events[0],
            ProviderEvent::ToolCallStarted { id, name }
                if id.as_str() == "ollama-call-0" && name.as_str() == "lookup"
        ));
        assert!(matches!(
            &events[1],
            ProviderEvent::ToolCallArgumentsDelta { id, delta }
                if id.as_str() == "ollama-call-0"
                    && delta.as_str() == r#"{"q":"seoul"}"#
        ));
        assert!(matches!(
            &events[2],
            ProviderEvent::ToolCallCompleted { id } if id.as_str() == "ollama-call-0"
        ));
    }

    #[tokio::test]
    async fn rejects_incomplete_malformed_and_streaming_error_responses() {
        let cases = [
            (
                br#"{"message":{"role":"assistant","content":"partial"},"done":false}
"#
                .as_slice(),
                "Ollama stream ended before its terminal response",
            ),
            (
                b"{not-json}\n".as_slice(),
                "Ollama returned malformed streaming data",
            ),
            (
                br#"{"error":"private vendor diagnostic: secret"}
"#
                .as_slice(),
                "Ollama returned a streaming error",
            ),
            (
                br#"{"unexpected":true}
"#
                .as_slice(),
                "Ollama returned malformed streaming data",
            ),
            (
                br#"{"message":{"content":"private-missing-role"},"done":true}
"#
                .as_slice(),
                "Ollama returned a non-assistant chat message",
            ),
            (
                br#"{"message":{"role":"user","content":"private-wrong-role"},"done":true}
"#
                .as_slice(),
                "Ollama returned a non-assistant chat message",
            ),
        ];

        for (body, expected_message) in cases {
            let (result, _, _) = generate_from_body(body, 3).await;
            let error = result.expect_err("invalid stream");
            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
            assert_eq!(error.message, expected_message);
            assert!(!error.message.contains("private vendor diagnostic"));
        }
    }

    #[tokio::test]
    async fn rejects_oversized_jsonl_line() {
        let mut body = vec![b' '; MAX_JSONL_LINE_BYTES + 1];
        body.push(b'\n');
        let (result, events, _) = generate_from_body(&body, 64 * 1024).await;
        let error = result.expect_err("oversized line");

        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(error.message, "Ollama streaming line exceeded 1 MiB");
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn rejects_non_ndjson_success_without_parsing_its_body() {
        let body = br#"{"message":{"role":"assistant","content":"private-body"},"done":true}"#;
        let (base_url, _captured) = synthetic_server(SyntheticResponse {
            status: "200 OK",
            content_type: "application/json",
            body: body.to_vec(),
            fragment_bytes: body.len(),
            extra_headers: "",
        });
        let provider =
            OllamaNativeProvider::new(&base_url, Duration::from_secs(2)).expect("provider");
        let (sink, mut events) = mpsc::channel(2);
        let (_cancel, cancelled) = watch::channel(false);

        let error = provider
            .generate(request(), None, sink, cancelled)
            .await
            .expect_err("wrong streaming content type");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(
            error.message,
            "Ollama returned an unexpected streaming content type"
        );
        assert!(!error.message.contains("private-body"));
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn ignores_unrequested_logprob_payloads_without_exposing_them() {
        let body = br#"{"message":{"role":"assistant","content":"safe"},"done":true,"logprobs":[{"token":"private-logprob-canary","logprob":-0.1}]}
"#;
        let (result, events, _) = generate_from_body(body, 5).await;
        let usage = result.expect("forward-compatible logprob response");
        assert_eq!(usage.provider_raw_summary, None);
        assert_eq!(events, vec![ProviderEvent::TextDelta("safe".to_owned())]);
        assert!(!format!("{usage:?}{events:?}").contains("private-logprob-canary"));
    }

    #[tokio::test]
    async fn list_models_parses_native_tags_response() {
        let body = br#"{
            "models": [{
                "name": "gemma3:latest",
                "model": "gemma3:latest",
                "modified_at": "2025-10-03T23:34:03Z",
                "size": 3338801804,
                "digest": "synthetic-digest",
                "details": {
                    "format": "gguf",
                    "family": "gemma",
                    "families": ["gemma"],
                    "parameter_size": "4.3B",
                    "quantization_level": "Q4_K_M"
                }
            }]
        }"#;
        let (base_url, captured) = synthetic_server(SyntheticResponse {
            status: "200 OK",
            content_type: "application/json",
            body: body.to_vec(),
            fragment_bytes: 5,
            extra_headers: "",
        });
        let provider =
            OllamaNativeProvider::new(&base_url, Duration::from_secs(2)).expect("provider");
        let (_cancel, cancelled) = watch::channel(false);

        let models = provider
            .list_models(None, cancelled)
            .await
            .expect("model list");

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model_id, "gemma3:latest");
        assert_eq!(models[0].display_name, "gemma3:latest");
        assert_eq!(models[0].size, Some(3_338_801_804));
        assert_eq!(
            models[0].details.as_ref().expect("details").families,
            vec!["gemma"]
        );
        let request =
            String::from_utf8(captured.recv().expect("captured request")).expect("UTF-8 request");
        assert!(request.starts_with("GET /api/tags HTTP/1.1\r\n"));
    }

    #[test]
    fn model_catalog_fields_are_individually_bounded() {
        let oversized = RawModelSummary {
            name: None,
            model: Some("m".repeat(MAX_MODEL_ID_BYTES + 1)),
            modified_at: None,
            size: None,
            digest: None,
            details: None,
        };
        let error = normalize_models(vec![oversized]).expect_err("oversized model id");
        assert_eq!(
            error.message,
            "Ollama model catalog contained an invalid model"
        );

        let excessive_families = RawModelSummary {
            name: Some("model".to_owned()),
            model: Some("model".to_owned()),
            modified_at: None,
            size: None,
            digest: None,
            details: Some(RawModelDetails {
                format: None,
                family: None,
                families: vec!["family".to_owned(); MAX_MODEL_FAMILIES + 1],
                parameter_size: None,
                quantization_level: None,
            }),
        };
        let error =
            normalize_models(vec![excessive_families]).expect_err("too many model families");
        assert_eq!(
            error.message,
            "Ollama model catalog contained invalid metadata"
        );
    }

    #[tokio::test]
    async fn list_models_bounds_the_catalog_body() {
        let (base_url, _captured) = synthetic_server(SyntheticResponse {
            status: "200 OK",
            content_type: "application/json",
            body: vec![b' '; MAX_MODEL_CATALOG_BYTES + 1],
            fragment_bytes: 64 * 1024,
            extra_headers: "",
        });
        let provider =
            OllamaNativeProvider::new(&base_url, Duration::from_secs(5)).expect("provider");
        let (_cancel, cancelled) = watch::channel(false);

        let error = provider
            .list_models(None, cancelled)
            .await
            .expect_err("oversized catalog");

        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(error.message, "Ollama model catalog exceeded 4 MiB");
    }

    #[tokio::test]
    async fn list_models_accepts_the_exact_catalog_bound() {
        let prefix = br#"{"models":[],"padding":""#;
        let suffix = br#""}"#;
        let padding = MAX_MODEL_CATALOG_BYTES - prefix.len() - suffix.len();
        let mut body = Vec::with_capacity(MAX_MODEL_CATALOG_BYTES);
        body.extend_from_slice(prefix);
        body.extend(std::iter::repeat_n(b'x', padding));
        body.extend_from_slice(suffix);
        assert_eq!(body.len(), MAX_MODEL_CATALOG_BYTES);
        let (base_url, _captured) = synthetic_server(SyntheticResponse {
            status: "200 OK",
            content_type: "application/json",
            body,
            fragment_bytes: 64 * 1024,
            extra_headers: "",
        });
        let provider =
            OllamaNativeProvider::new(&base_url, Duration::from_secs(5)).expect("provider");
        let (_cancel, cancelled) = watch::channel(false);

        let models = provider
            .list_models(None, cancelled)
            .await
            .expect("catalog at exact bound");

        assert!(models.is_empty());
    }

    #[tokio::test]
    async fn list_models_observes_cancellation_while_receiving_the_body() {
        let (base_url, started) = stalling_catalog_server();
        let provider =
            OllamaNativeProvider::new(&base_url, Duration::from_secs(5)).expect("provider");
        let (cancel, cancelled) = watch::channel(false);
        let listing = tokio::spawn(async move { provider.list_models(None, cancelled).await });

        let wait_started = Instant::now();
        loop {
            if started.try_recv().is_ok() {
                break;
            }
            assert!(
                wait_started.elapsed() < Duration::from_secs(1),
                "catalog response did not start"
            );
            tokio::task::yield_now().await;
        }
        cancel.send(true).expect("cancel model listing");

        let error = listing
            .await
            .expect("listing task")
            .expect_err("cancelled model listing");

        assert_eq!(error.code, CoreErrorCode::Cancelled);
    }

    #[tokio::test]
    async fn maps_statuses_and_does_not_follow_redirects() {
        for (status, expected_code, recoverable) in [
            ("400 Bad Request", CoreErrorCode::InvalidInput, false),
            ("401 Unauthorized", CoreErrorCode::ProviderAuthFailed, false),
            ("404 Not Found", CoreErrorCode::NotFound, false),
            (
                "429 Too Many Requests",
                CoreErrorCode::ProviderRateLimited,
                true,
            ),
            (
                "500 Internal Server Error",
                CoreErrorCode::ProviderUnavailable,
                true,
            ),
            ("302 Found", CoreErrorCode::ProviderUnavailable, false),
        ] {
            let (base_url, _) = synthetic_server(SyntheticResponse {
                status,
                content_type: "application/json",
                body: Vec::new(),
                fragment_bytes: 1,
                extra_headers: "Location: https://example.invalid/redirect\r\n",
            });
            let provider =
                OllamaNativeProvider::new(&base_url, Duration::from_secs(2)).expect("provider");
            let (sink, _events) = mpsc::channel(1);
            let (_cancel, cancelled) = watch::channel(false);
            let error = provider
                .generate(request(), None, sink, cancelled)
                .await
                .expect_err("status error");

            assert_eq!(error.code, expected_code);
            assert_eq!(error.recoverable, recoverable);
        }
    }

    #[tokio::test]
    async fn cancellation_is_observed_before_connecting() {
        let provider = OllamaNativeProvider::new("http://127.0.0.1:9", Duration::from_secs(2))
            .expect("provider");
        let (sink, _events) = mpsc::channel(1);
        let (cancel, cancelled) = watch::channel(false);
        cancel.send(true).expect("cancel");

        let error = provider
            .generate(request(), None, sink, cancelled)
            .await
            .expect_err("cancelled");

        assert_eq!(error.code, CoreErrorCode::Cancelled);
    }

    #[tokio::test]
    async fn invalid_generation_bounds_are_rejected_before_connecting() {
        let provider = OllamaNativeProvider::new("http://127.0.0.1:9", Duration::from_secs(2))
            .expect("provider");
        for invalid in [
            GenerationRequest {
                model: " \t".to_owned(),
                ..request()
            },
            GenerationRequest {
                max_output_tokens: Some(0),
                ..request()
            },
            GenerationRequest {
                temperature: Some(-0.1),
                ..request()
            },
            GenerationRequest {
                messages: Vec::new(),
                ..request()
            },
            GenerationRequest {
                model: "m".repeat(MAX_MODEL_ID_BYTES + 1),
                ..request()
            },
        ] {
            let (sink, _events) = mpsc::channel(1);
            let (_cancel, cancelled) = watch::channel(false);
            let error = provider
                .generate(invalid, None, sink, cancelled)
                .await
                .expect_err("invalid request");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
        }

        let mut wrong_conversation = request();
        wrong_conversation
            .messages
            .push(Message::user(ConversationId::new(), "wrong conversation"));
        assert_eq!(
            validate_request(&wrong_conversation)
                .expect_err("cross-conversation prompt")
                .code,
            CoreErrorCode::InvalidInput
        );

        let mut oversized = request();
        oversized.messages[0].content = "p".repeat(MAX_PROMPT_BYTES + 1);
        assert_eq!(
            validate_request(&oversized)
                .expect_err("oversized prompt")
                .code,
            CoreErrorCode::InvalidInput
        );

        let mut empty_opaque_preference = request();
        empty_opaque_preference.preserve_opaque_reasoning_state = true;
        let (sink, _events) = mpsc::channel(1);
        let (_cancel, cancelled) = watch::channel(false);
        let error = provider
            .generate(empty_opaque_preference.clone(), None, sink, cancelled)
            .await
            .expect_err("Ollama cannot preserve opaque provider state");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);

        let mut incompatible_opaque_context = empty_opaque_preference;
        incompatible_opaque_context.preserve_opaque_reasoning_state = false;
        incompatible_opaque_context.opaque_reasoning_context = vec![OpaqueReasoningContext {
            source_message_id: incompatible_opaque_context.messages[0].id.clone(),
            api_family: ApiFamily::GeminiGenerateContent,
            model: incompatible_opaque_context.model.clone(),
            model_route_id: ModelRouteId::from("other-route"),
            generation_preset_id: GenerationPresetId::from("other-preset"),
            state: OpaqueReasoningState::GeminiThoughtSignature {
                part_index: 0,
                signature: OpaqueReasoningData::parse("private-signature")
                    .expect("bounded signature"),
            },
        }];
        let (sink, _events) = mpsc::channel(1);
        let (_cancel, cancelled) = watch::channel(false);
        let error = provider
            .generate(incompatible_opaque_context, None, sink, cancelled)
            .await
            .expect_err("Ollama cannot replay opaque provider state");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(!error.message.contains("private-signature"));
    }

    #[tokio::test]
    async fn cancellation_interrupts_buffered_event_delivery() {
        let body = concat!(
            "{\"message\":{\"role\":\"assistant\",\"content\":\"first\"},\"done\":false}\n",
            "{\"message\":{\"role\":\"assistant\",\"content\":\"second\"},\"done\":false}\n",
            "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true}\n",
        );
        let (base_url, _) = synthetic_server(SyntheticResponse {
            status: "200 OK",
            content_type: "application/x-ndjson",
            body: body.as_bytes().to_vec(),
            fragment_bytes: body.len(),
            extra_headers: "",
        });
        let provider =
            OllamaNativeProvider::new(&base_url, Duration::from_secs(2)).expect("provider");
        let (sink, mut events) = mpsc::channel(1);
        let (cancel, cancelled) = watch::channel(false);
        let generation =
            tokio::spawn(async move { provider.generate(request(), None, sink, cancelled).await });

        tokio::time::timeout(Duration::from_secs(1), async {
            while events.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first event");
        cancel.send(true).expect("cancel");

        let error = generation
            .await
            .expect("generation task")
            .expect_err("cancelled generation");
        assert_eq!(error.code, CoreErrorCode::Cancelled);
        assert_eq!(
            events.try_recv().expect("first event"),
            ProviderEvent::TextDelta("first".to_owned())
        );
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn constructs_api_paths_without_duplicating_api_segment() {
        let root = OllamaNativeProvider::new("http://localhost:11434", Duration::from_secs(1))
            .expect("root provider");
        assert_eq!(root.chat_endpoint.path(), "/api/chat");
        assert_eq!(
            root.tags_endpoint
                .as_ref()
                .expect("direct provider has a tags endpoint")
                .path(),
            "/api/tags"
        );

        let api = OllamaNativeProvider::new("http://localhost:11434/api/", Duration::from_secs(1))
            .expect("API provider");
        assert_eq!(api.chat_endpoint.path(), "/api/chat");

        let prefixed =
            OllamaNativeProvider::new("https://ollama.com/ollama", Duration::from_secs(1))
                .expect("prefixed provider");
        assert_eq!(prefixed.chat_endpoint.path(), "/ollama/api/chat");

        let encoded =
            OllamaNativeProvider::new("https://ollama.com/reverse%20proxy", Duration::from_secs(1))
                .expect("percent-encoded reverse-proxy prefix");
        assert_eq!(
            encoded.chat_endpoint.as_str(),
            "https://ollama.com/reverse%20proxy/api/chat"
        );
        assert!(!encoded.chat_endpoint.as_str().contains("%2520"));
    }

    #[test]
    fn enforces_jsonl_line_and_total_stream_bounds() {
        assert!(ensure_jsonl_line_size(MAX_JSONL_LINE_BYTES).is_ok());
        assert_eq!(
            ensure_jsonl_line_size(MAX_JSONL_LINE_BYTES + 1)
                .expect_err("oversized line")
                .message,
            "Ollama streaming line exceeded 1 MiB"
        );
        assert!(ensure_stream_size(MAX_STREAM_BYTES).is_ok());
        assert_eq!(
            ensure_stream_size(MAX_STREAM_BYTES + 1)
                .expect_err("oversized stream")
                .message,
            "Ollama stream exceeded 64 MiB"
        );
    }
}
