use std::{collections::BTreeMap, net::IpAddr, time::Duration};

use async_trait::async_trait;
use futures_util::StreamExt;
use lorepia_domain::{
    ApiFamily, AuthBinding, CoreError, CoreErrorCode, CoreResult, GenerationRequest,
    GenerationUsage, MessageRole, ProviderCapabilities, ToolCallArgumentsDelta, ToolCallId,
    ToolName,
};
use reqwest::{
    StatusCode,
    header::{ACCEPT, CONTENT_TYPE},
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
};

const MAX_SSE_EVENT_BYTES: usize = 1024 * 1024;
const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
const MAX_PROMPT_MESSAGES: usize = 128;
const MAX_PROMPT_BYTES: usize = 512 * 1024;
const MAX_PROMPT_CHARS: usize = 128 * 1024;
const MAX_MODEL_ID_BYTES: usize = 1024;
const MAX_MODEL_ID_CHARS: usize = 256;
const MAX_PENDING_TOOL_ARGUMENT_BYTES: usize = 256 * 1024;
const SSE_EVENT_SEPARATORS: [&[u8]; 8] = [
    b"\r\n\r\n",
    b"\n\r\n",
    b"\r\r\n",
    b"\r\n\n",
    b"\r\n\r",
    b"\n\n",
    b"\n\r",
    b"\r\r",
];

/// Streaming provider for the `OpenAI` Responses API.
#[derive(Clone)]
pub struct OpenAiResponsesProvider {
    endpoint: Url,
    target: ProviderHttpTarget,
    auth: AuthBinding,
    request_plan: Option<ProviderRequestPlan>,
}

impl OpenAiResponsesProvider {
    /// Creates an adapter whose base URL is the API root, for example
    /// `https://api.openai.com/v1`.
    pub fn new(base_url: &str, timeout: Duration) -> CoreResult<Self> {
        if timeout.is_zero() {
            return Err(CoreError::invalid(
                "provider timeout must be greater than zero",
            ));
        }

        let mut endpoint =
            Url::parse(base_url).map_err(|_| CoreError::invalid("invalid provider base URL"))?;
        validate_endpoint(&endpoint)?;
        if endpoint.query().is_some() || endpoint.fragment().is_some() {
            return Err(CoreError::invalid(
                "provider base URL must not contain a query or fragment",
            ));
        }
        if !endpoint.path().ends_with('/') {
            let path = format!("{}/", endpoint.path());
            endpoint.set_path(&path);
        }
        endpoint = endpoint
            .join("responses")
            .map_err(|_| CoreError::invalid("cannot construct OpenAI Responses API endpoint"))?;
        validate_endpoint(&endpoint)?;

        let target = ProviderHttpTarget::inferred(endpoint.as_str(), timeout)?;
        Ok(Self {
            endpoint,
            target,
            auth: AuthBinding::BearerHeader,
            request_plan: None,
        })
    }

    pub(crate) fn new_with_manifest_target(target: ProviderHttpTarget, auth: AuthBinding) -> Self {
        // `ProviderHttpTarget` has already canonicalized this endpoint against
        // the caller's exact network policy. Reapplying the public/loopback
        // convenience-constructor policy here would incorrectly reject an
        // explicitly approved private-LAN target.
        Self {
            endpoint: target.url().clone(),
            target,
            auth,
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
}

#[async_trait]
impl Provider for OpenAiResponsesProvider {
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
        let payload = request_payload(request.clone(), cache_dialect)?;
        planned_json_payload(
            &payload,
            ApiFamily::OpenAiResponses,
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
        validate_credential_for_auth(&self.auth, credential)?;

        let cache_dialect = self
            .request_plan
            .as_ref()
            .map_or(PromptCacheWireDialect::Unsupported, |plan| {
                plan.cache_dialect()
            });
        let payload = request_payload(request, cache_dialect)?;
        let payload = planned_json_payload(
            &payload,
            ApiFamily::OpenAiResponses,
            self.request_plan.as_ref(),
        )?;
        let prepared = self.target.prepare().await?;
        ensure_not_cancelled(&cancelled)?;
        let request = authorize_request(
            prepared
                .client()
                .post(self.endpoint.clone())
                .header(ACCEPT, "text/event-stream")
                .json(&payload),
            &self.auth,
            credential,
        )?;
        let response = request.send();
        tokio::pin!(response);

        let mut cancellation_open = true;
        let response = loop {
            tokio::select! {
                biased;
                change = cancelled.changed(), if cancellation_open => {
                    if change.is_ok() {
                        ensure_not_cancelled(&cancelled)?;
                    } else {
                        cancellation_open = false;
                    }
                }
                result = &mut response => {
                    break result.map_err(network_error)?;
                }
            }
        };

        prepared.validate_response_peer(&response)?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        ensure_event_stream_content_type(&response)?;
        consume_response_stream(response, &sink, &mut cancelled, &mut cancellation_open).await
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

async fn consume_response_stream(
    response: reqwest::Response,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<GenerationUsage> {
    let mut bytes = response.bytes_stream();
    let mut pending = Vec::<u8>::new();
    let mut usage = GenerationUsage::default();
    let mut total_stream_bytes = 0_usize;
    let mut tool_calls = ResponsesToolCallTracker::default();

    loop {
        tokio::select! {
            biased;
            change = cancelled.changed(), if *cancellation_open => {
                if change.is_ok() {
                    ensure_not_cancelled(cancelled)?;
                } else {
                    *cancellation_open = false;
                }
            }
            chunk = bytes.next() => {
                let Some(chunk) = chunk else { break };
                let chunk = chunk.map_err(network_error)?;
                total_stream_bytes = total_stream_bytes
                    .checked_add(chunk.len())
                    .ok_or_else(stream_too_large_error)?;
                ensure_stream_size(total_stream_bytes)?;
                pending.extend_from_slice(&chunk);

                if chunk
                    .iter()
                    .any(|byte| matches!(*byte, b'\r' | b'\n'))
                {
                    while let Some((boundary, separator_len)) =
                        find_event_boundary(&pending, false)
                    {
                        ensure_not_cancelled(cancelled)?;
                        ensure_event_size(boundary)?;
                        let event = pending.drain(..boundary).collect::<Vec<_>>();
                        pending.drain(..separator_len);
                        if process_event(
                            &event,
                            sink,
                            &mut usage,
                            &mut tool_calls,
                            cancelled,
                            cancellation_open,
                        )
                        .await?
                            == EventAction::Complete
                        {
                            ensure_not_cancelled(cancelled)?;
                            return Ok(usage);
                        }
                    }
                }
                ensure_pending_size(&pending, false)?;
            }
        }
    }

    while let Some((boundary, separator_len)) = find_event_boundary(&pending, true) {
        ensure_not_cancelled(cancelled)?;
        ensure_event_size(boundary)?;
        let event = pending.drain(..boundary).collect::<Vec<_>>();
        pending.drain(..separator_len);
        if process_event(
            &event,
            sink,
            &mut usage,
            &mut tool_calls,
            cancelled,
            cancellation_open,
        )
        .await?
            == EventAction::Complete
        {
            ensure_not_cancelled(cancelled)?;
            return Ok(usage);
        }
    }
    ensure_pending_size(&pending, true)?;
    if !pending.is_empty() {
        return Err(streaming_error(
            "provider stream ended with an incomplete event",
        ));
    }
    Err(streaming_error(
        "provider stream ended before response.completed",
    ))
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
            "provider model is not a bounded identifier",
        ));
    }
    validate_prompt_bounds(request)?;
    if request.temperature.is_some_and(|value| !value.is_finite()) {
        return Err(CoreError::invalid(
            "provider temperature must be a finite number",
        ));
    }
    if request.max_output_tokens == Some(0) {
        return Err(CoreError::invalid(
            "provider max output tokens must be greater than zero",
        ));
    }
    if request.preserve_opaque_reasoning_state || !request.opaque_reasoning_context.is_empty() {
        return Err(CoreError::invalid(
            "OpenAI Responses opaque reasoning continuity is unsupported",
        ));
    }
    Ok(())
}

fn validate_prompt_bounds(request: &GenerationRequest) -> CoreResult<()> {
    let prompt_message_count = request
        .resolved_prompt_plan
        .as_ref()
        .map_or(request.messages.len(), |plan| plan.effective_messages.len());
    if prompt_message_count > MAX_PROMPT_MESSAGES {
        return Err(CoreError::invalid(
            "provider prompt exceeded the message-count safety limit",
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
                    "provider prompt contains a message from another conversation",
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
    Ok(())
}

fn prompt_too_large_error() -> CoreError {
    CoreError::invalid("provider prompt exceeded the input safety limit")
}

fn request_payload(
    request: GenerationRequest,
    cache_dialect: PromptCacheWireDialect,
) -> CoreResult<RequestPayload> {
    if request.preserve_opaque_reasoning_state || !request.opaque_reasoning_context.is_empty() {
        return Err(CoreError::invalid(
            "OpenAI Responses opaque reasoning continuity is unsupported",
        ));
    }
    let input = if let Some(plan) = &request.resolved_prompt_plan {
        let compiled = prompt_contract()
            .compile_resolved_plan_for_execution(plan, cache_dialect)
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        compiled
            .verify_execution_hash(request.provider_execution_plan_hash.as_deref())
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        compiled
            .messages()
            .iter()
            .map(|message| RequestMessage {
                kind: "message",
                role: message.wire_role().as_str(),
                content: message.content().to_owned(),
            })
            .collect()
    } else {
        request
            .messages
            .into_iter()
            .map(|message| RequestMessage {
                kind: "message",
                role: role_name(message.role),
                content: message.content,
            })
            .collect()
    };
    Ok(RequestPayload {
        model: request.model,
        input,
        stream: true,
        store: false,
        temperature: request.temperature,
        max_output_tokens: request.max_output_tokens,
    })
}

#[derive(Serialize)]
struct RequestPayload {
    model: String,
    input: Vec<RequestMessage>,
    stream: bool,
    store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

#[derive(Serialize)]
struct RequestMessage {
    #[serde(rename = "type")]
    kind: &'static str,
    role: &'static str,
    content: String,
}

#[derive(Deserialize)]
struct DeltaEvent {
    delta: String,
}

#[derive(Deserialize)]
struct CompletedEvent {
    response: CompletedResponse,
}

#[derive(Deserialize)]
struct CompletedResponse {
    status: String,
    usage: Option<ResponseUsage>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ResponseUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    input_tokens_details: Option<ResponseInputTokensDetails>,
    output_tokens_details: Option<ResponseOutputTokensDetails>,
}

#[derive(Deserialize)]
struct ResponseInputTokensDetails {
    cached_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct ResponseOutputTokensDetails {
    reasoning_tokens: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
enum EventAction {
    Continue,
    Complete,
}

#[derive(Default)]
struct ResponsesToolCallTracker {
    calls: BTreeMap<String, ResponsesToolCall>,
}

struct ResponsesToolCall {
    id: ToolCallId,
    name: ToolName,
    arguments: String,
    completed: bool,
}

#[allow(clippy::too_many_lines)]
async fn process_event(
    event: &[u8],
    sink: &ProviderEventSender,
    usage: &mut GenerationUsage,
    tool_calls: &mut ResponsesToolCallTracker,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<EventAction> {
    let Some(data) = event_data(event)? else {
        return Ok(EventAction::Continue);
    };
    if data == "[DONE]" {
        return Err(streaming_error(
            "provider stream ended before response.completed",
        ));
    }

    let value: serde_json::Value = serde_json::from_str(&data)
        .map_err(|_| streaming_error("provider returned malformed streaming data"))?;
    let event_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| streaming_error("provider returned malformed streaming data"))?;

    match event_type {
        "response.output_text.delta" | "response.refusal.delta" => {
            let event: DeltaEvent = serde_json::from_value(value)
                .map_err(|_| streaming_error("provider returned malformed streaming data"))?;
            if !event.delta.is_empty() {
                send_provider_event(
                    sink,
                    ProviderEvent::TextDelta(event.delta),
                    cancelled,
                    cancellation_open,
                )
                .await?;
            }
            Ok(EventAction::Continue)
        }
        "response.reasoning_summary_text.delta" => {
            let event: DeltaEvent = serde_json::from_value(value)
                .map_err(|_| streaming_error("provider returned malformed streaming data"))?;
            if !event.delta.is_empty() {
                send_provider_event(
                    sink,
                    ProviderEvent::ReasoningDelta(event.delta),
                    cancelled,
                    cancellation_open,
                )
                .await?;
            }
            Ok(EventAction::Continue)
        }
        "response.output_item.added"
            if value
                .get("item")
                .and_then(|item| item.get("type"))
                .and_then(serde_json::Value::as_str)
                == Some("function_call") =>
        {
            let item = value
                .get("item")
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| streaming_error("provider returned malformed tool-call data"))?;
            let item_id = item
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| streaming_error("provider returned malformed tool-call data"))?;
            let call_id = item
                .get("call_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| streaming_error("provider returned malformed tool-call data"))?;
            let name = item
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| streaming_error("provider returned malformed tool-call data"))?;
            let id = ToolCallId::parse(call_id.to_owned())
                .map_err(|_| streaming_error("provider returned invalid tool-call data"))?;
            let name = ToolName::parse(name.to_owned())
                .map_err(|_| streaming_error("provider returned invalid tool-call data"))?;
            if tool_calls
                .calls
                .insert(
                    item_id.to_owned(),
                    ResponsesToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: String::new(),
                        completed: false,
                    },
                )
                .is_some()
            {
                return Err(streaming_error(
                    "provider returned duplicate tool-call data",
                ));
            }
            send_provider_event(
                sink,
                ProviderEvent::ToolCallStarted { id, name },
                cancelled,
                cancellation_open,
            )
            .await?;
            Ok(EventAction::Continue)
        }
        "response.function_call_arguments.delta" => {
            let item_id = value
                .get("item_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| streaming_error("provider returned malformed tool-call data"))?;
            let arguments = value
                .get("delta")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| streaming_error("provider returned malformed tool-call data"))?;
            let call = tool_calls
                .calls
                .get_mut(item_id)
                .filter(|call| !call.completed)
                .ok_or_else(|| streaming_error("provider returned unknown tool-call data"))?;
            append_tool_arguments(&mut call.arguments, arguments)?;
            if !arguments.is_empty() {
                let delta = ToolCallArgumentsDelta::parse(arguments.to_owned())
                    .map_err(|_| streaming_error("provider returned invalid tool-call data"))?;
                send_provider_event(
                    sink,
                    ProviderEvent::ToolCallArgumentsDelta {
                        id: call.id.clone(),
                        delta,
                    },
                    cancelled,
                    cancellation_open,
                )
                .await?;
            }
            Ok(EventAction::Continue)
        }
        "response.function_call_arguments.done" => {
            let item_id = value
                .get("item_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| streaming_error("provider returned malformed tool-call data"))?;
            let name = value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| streaming_error("provider returned malformed tool-call data"))?;
            let arguments = value
                .get("arguments")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| streaming_error("provider returned malformed tool-call data"))?;
            let call = tool_calls
                .calls
                .get_mut(item_id)
                .filter(|call| !call.completed && call.name.as_str() == name)
                .ok_or_else(|| streaming_error("provider returned unknown tool-call data"))?;
            if call.arguments.is_empty() && !arguments.is_empty() {
                append_tool_arguments(&mut call.arguments, arguments)?;
                let delta = ToolCallArgumentsDelta::parse(arguments.to_owned())
                    .map_err(|_| streaming_error("provider returned invalid tool-call data"))?;
                send_provider_event(
                    sink,
                    ProviderEvent::ToolCallArgumentsDelta {
                        id: call.id.clone(),
                        delta,
                    },
                    cancelled,
                    cancellation_open,
                )
                .await?;
            } else if call.arguments != arguments {
                return Err(streaming_error(
                    "provider changed completed tool-call arguments",
                ));
            }
            call.completed = true;
            send_provider_event(
                sink,
                ProviderEvent::ToolCallCompleted {
                    id: call.id.clone(),
                },
                cancelled,
                cancellation_open,
            )
            .await?;
            Ok(EventAction::Continue)
        }
        "response.completed" => {
            let event: CompletedEvent = serde_json::from_value(value)
                .map_err(|_| streaming_error("provider returned malformed streaming data"))?;
            if event.response.status != "completed"
                || event
                    .response
                    .error
                    .as_ref()
                    .is_some_and(|error| !error.is_null())
            {
                return Err(streaming_error(
                    "provider returned an invalid completion event",
                ));
            }
            if tool_calls.calls.values().any(|call| !call.completed) {
                return Err(streaming_error(
                    "provider completed with an unfinished tool call",
                ));
            }
            update_usage(event.response.usage.as_ref(), usage);
            Ok(EventAction::Complete)
        }
        "response.failed" | "response.incomplete" | "response.cancelled" | "error" => {
            Err(streaming_error("provider reported a streaming failure"))
        }
        // Unknown events, including raw reasoning text, are intentionally not
        // exposed. Only user-visible reasoning summaries are forwarded.
        _ => Ok(EventAction::Continue),
    }
}

fn append_tool_arguments(target: &mut String, fragment: &str) -> CoreResult<()> {
    target
        .len()
        .checked_add(fragment.len())
        .filter(|size| *size <= MAX_PENDING_TOOL_ARGUMENT_BYTES)
        .ok_or_else(|| streaming_error("provider tool-call arguments exceeded its safety limit"))?;
    target.push_str(fragment);
    Ok(())
}

fn event_data(event: &[u8]) -> CoreResult<Option<String>> {
    let text = std::str::from_utf8(event)
        .map_err(|_| streaming_error("provider returned malformed streaming data"))?;
    let mut data_lines = Vec::new();
    for line in text.split(['\r', '\n']) {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        data_lines.push(data.strip_prefix(' ').unwrap_or(data));
    }
    if data_lines.is_empty() {
        return Ok(None);
    }

    let data = data_lines.join("\n");
    let data = data.trim();
    if data.is_empty() {
        Ok(None)
    } else {
        Ok(Some(data.to_owned()))
    }
}

fn update_usage(response_usage: Option<&ResponseUsage>, usage: &mut GenerationUsage) {
    if let Some(response_usage) = response_usage {
        usage.input_tokens = response_usage.input_tokens.or(usage.input_tokens);
        usage.output_tokens = response_usage.output_tokens.or(usage.output_tokens);
        usage.cached_read_tokens = response_usage
            .input_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens)
            .or(usage.cached_read_tokens);
        usage.cached_write_tokens = response_usage
            .input_tokens_details
            .as_ref()
            .and_then(|details| details.cache_write_tokens)
            .or(usage.cached_write_tokens);
        usage.reasoning_tokens = response_usage
            .output_tokens_details
            .as_ref()
            .and_then(|details| details.reasoning_tokens)
            .or(usage.reasoning_tokens);
        usage.provider_raw_summary = merge_usage_summary(
            usage.provider_raw_summary.as_ref(),
            &[("total_tokens", response_usage.total_tokens)],
        );
    }
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

fn ensure_event_stream_content_type(response: &reqwest::Response) -> CoreResult<()> {
    let is_event_stream = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/event-stream"));
    if is_event_stream {
        Ok(())
    } else {
        Err(streaming_error(
            "provider returned a non-streaming response",
        ))
    }
}

fn find_event_boundary(bytes: &[u8], end_of_stream: bool) -> Option<(usize, usize)> {
    for position in 0..bytes.len() {
        for separator in SSE_EVENT_SEPARATORS {
            let ends_at_buffer_edge = position + separator.len() == bytes.len();
            if bytes[position..].starts_with(separator)
                && (end_of_stream
                    || !separator.ends_with(b"\r")
                    || separator == b"\r\r"
                    || !ends_at_buffer_edge)
            {
                return Some((position, separator.len()));
            }
        }
    }
    None
}

fn ensure_event_size(size: usize) -> CoreResult<()> {
    if size > MAX_SSE_EVENT_BYTES {
        Err(streaming_error("provider streaming event exceeded 1 MiB"))
    } else {
        Ok(())
    }
}

fn ensure_pending_size(bytes: &[u8], end_of_stream: bool) -> CoreResult<()> {
    if bytes.len() <= MAX_SSE_EVENT_BYTES {
        return Ok(());
    }
    let possible_separator = &bytes[MAX_SSE_EVENT_BYTES..];
    if !end_of_stream
        && SSE_EVENT_SEPARATORS.iter().any(|separator| {
            possible_separator.len() < separator.len() && separator.starts_with(possible_separator)
        })
    {
        return Ok(());
    }
    Err(streaming_error("provider streaming event exceeded 1 MiB"))
}

fn ensure_stream_size(size: usize) -> CoreResult<()> {
    if size > MAX_STREAM_BYTES {
        Err(stream_too_large_error())
    } else {
        Ok(())
    }
}

fn stream_too_large_error() -> CoreError {
    streaming_error("provider stream exceeded 64 MiB")
}

fn validate_endpoint(endpoint: &Url) -> CoreResult<()> {
    if endpoint.cannot_be_a_base() || endpoint.host().is_none() {
        return Err(CoreError::invalid(
            "provider base URL must be an absolute network URL",
        ));
    }
    if endpoint.username() != "" || endpoint.password().is_some() {
        return Err(CoreError::invalid(
            "provider URL must not contain embedded credentials",
        ));
    }
    match endpoint.scheme() {
        "https" => Ok(()),
        "http" if endpoint.host().is_some_and(is_loopback_host) => Ok(()),
        "http" => Err(CoreError::invalid(
            "unencrypted HTTP is allowed only for loopback endpoints",
        )),
        _ => Err(CoreError::invalid(
            "provider endpoint must use HTTPS or loopback HTTP",
        )),
    }
}

fn is_loopback_host(host: Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.');
            domain.eq_ignore_ascii_case("localhost")
                || domain
                    .to_ascii_lowercase()
                    .strip_suffix(".localhost")
                    .is_some_and(|prefix| !prefix.is_empty())
        }
        Host::Ipv4(address) => IpAddr::V4(address).is_loopback(),
        Host::Ipv6(address) => IpAddr::V6(address).is_loopback(),
    }
}

pub(crate) const fn prompt_contract() -> ProviderPromptAdapterContract {
    ProviderPromptAdapterContract::for_family(ApiFamily::OpenAiResponses)
}

#[cfg(test)]
pub(crate) fn resolved_prompt_payload_for_contract_test(
    request: GenerationRequest,
    cache_dialect: PromptCacheWireDialect,
) -> CoreResult<serde_json::Value> {
    let payload = request_payload(request, cache_dialect)?;
    serde_json::to_value(payload)
        .map_err(|_| CoreError::internal("failed to serialize OpenAI Responses test payload"))
}

fn role_name(role: MessageRole) -> &'static str {
    prompt_contract().map_message_role(role).wire_role.as_str()
}

fn ensure_not_cancelled(cancelled: &watch::Receiver<bool>) -> CoreResult<()> {
    if *cancelled.borrow() {
        Err(cancelled_error())
    } else {
        Ok(())
    }
}

fn cancelled_error() -> CoreError {
    CoreError::new(CoreErrorCode::Cancelled, "generation was cancelled", true)
}

fn streaming_error(message: &'static str) -> CoreError {
    CoreError::new(CoreErrorCode::ProviderUnavailable, message, true)
}

fn network_error(error: reqwest::Error) -> CoreError {
    if error.is_timeout() {
        CoreError::new(
            CoreErrorCode::ProviderUnavailable,
            "provider request timed out",
            true,
        )
    } else {
        CoreError::new(
            CoreErrorCode::NetworkUnavailable,
            "provider request failed",
            true,
        )
    }
}

fn status_error(status: StatusCode) -> CoreError {
    let (code, recoverable) = match status {
        StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE => {
            (CoreErrorCode::InvalidInput, false)
        }
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            (CoreErrorCode::ProviderAuthFailed, false)
        }
        StatusCode::TOO_MANY_REQUESTS => (CoreErrorCode::ProviderRateLimited, true),
        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_EARLY => {
            (CoreErrorCode::ProviderUnavailable, true)
        }
        _ => (CoreErrorCode::ProviderUnavailable, status.is_server_error()),
    };
    CoreError::new(
        code,
        format!("provider returned HTTP {}", status.as_u16()),
        recoverable,
    )
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc as std_mpsc,
        thread,
    };

    use lorepia_domain::{
        ApiFamily, ConversationId, GenerationId, GenerationPresetId, GenerationRequest,
        GenerationUsage, Message, MessageRole, ModelRouteId, OpaqueReasoningContext,
        OpaqueReasoningState, OpenAiResponsesReasoningItem,
    };
    use serde_json::Value;
    use tokio::sync::{mpsc, watch};

    use super::*;

    const TEST_CREDENTIAL: &str = "synthetic-test-token";

    struct CapturedRequest {
        head: String,
        body: Value,
    }

    fn request() -> GenerationRequest {
        let conversation_id = ConversationId::new();
        let mut system = Message::user(conversation_id.clone(), "system instruction");
        system.role = MessageRole::System;
        let user = Message::user(conversation_id.clone(), "hello");
        let mut assistant = Message::user(conversation_id.clone(), "prior answer");
        assistant.role = MessageRole::Assistant;
        GenerationRequest {
            generation_id: GenerationId::new(),
            conversation_id,
            model: "fixture-model".to_owned(),
            messages: vec![system, user, assistant],
            resolved_prompt_plan: None,
            provider_execution_plan_hash: None,
            temperature: Some(0.25),
            max_output_tokens: Some(321),
            provider_provenance: None,
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        }
    }

    fn reasoning_item(id: &str, encrypted_content: &str) -> OpenAiResponsesReasoningItem {
        OpenAiResponsesReasoningItem::from_value(&serde_json::json!({
            "id": id,
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": "fixture summary"}],
            "content": [{"type": "reasoning_text", "text": "fixture reasoning"}],
            "encrypted_content": encrypted_content,
            "status": "completed"
        }))
        .expect("reasoning item")
    }

    fn read_http_request(stream: &mut TcpStream) -> io::Result<CapturedRequest> {
        const MAX_REQUEST_BYTES: usize = 64 * 1024;

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
                    if name.eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
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

        let head = std::str::from_utf8(&request[..headers_len])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .to_owned();
        let body = serde_json::from_slice(&request[headers_len..headers_len + content_len])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(CapturedRequest { head, body })
    }

    fn stream_server(
        body: &[u8],
        fragment_bytes: usize,
    ) -> (String, std_mpsc::Receiver<CapturedRequest>) {
        stream_server_with_content_type(body, fragment_bytes, "text/event-stream")
    }

    fn stream_server_with_content_type(
        body: &[u8],
        fragment_bytes: usize,
        content_type: &str,
    ) -> (String, std_mpsc::Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stream server");
        let address = listener.local_addr().expect("stream server address");
        let chunks = body
            .chunks(fragment_bytes.max(1))
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>();
        let content_type = content_type.to_owned();
        let (capture_tx, capture_rx) = std_mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let request = read_http_request(&mut stream).expect("read request");
            capture_tx.send(request).expect("capture request");
            if write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            )
            .is_err()
            {
                return;
            }
            for chunk in chunks {
                if write!(stream, "{:X}\r\n", chunk.len()).is_err()
                    || stream.write_all(&chunk).is_err()
                    || stream.write_all(b"\r\n").is_err()
                {
                    return;
                }
            }
            let _ = stream.write_all(b"0\r\n\r\n");
        });
        (format!("http://{address}/v1"), capture_rx)
    }

    fn status_server(status: &str, extra_headers: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind status server");
        let address = listener.local_addr().expect("status server address");
        let status = status.to_owned();
        let extra_headers = extra_headers.to_owned();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            read_http_request(&mut stream).expect("read request");
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n{extra_headers}\r\n"
            )
            .expect("write status");
        });
        format!("http://{address}/v1")
    }

    async fn generate_from_stream(
        body: &[u8],
        fragment_bytes: usize,
    ) -> (CoreResult<GenerationUsage>, Vec<ProviderEvent>) {
        let (base_url, _capture) = stream_server(body, fragment_bytes);
        let provider =
            OpenAiResponsesProvider::new(&base_url, Duration::from_secs(2)).expect("provider");
        let (sink, mut events) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let result = provider
            .generate(request(), Some(TEST_CREDENTIAL), sink, cancelled)
            .await;
        let mut received = Vec::new();
        while let Ok(event) = events.try_recv() {
            received.push(event);
        }
        (result, received)
    }

    async fn status_error_from(status: &str, extra_headers: &str) -> CoreError {
        let provider = OpenAiResponsesProvider::new(
            &status_server(status, extra_headers),
            Duration::from_secs(2),
        )
        .expect("provider");
        let (sink, _events) = mpsc::channel(4);
        let (_cancel, cancelled) = watch::channel(false);
        provider
            .generate(request(), Some(TEST_CREDENTIAL), sink, cancelled)
            .await
            .expect_err("status must fail")
    }

    #[test]
    fn accepts_https_and_loopback_http_only() {
        assert!(
            OpenAiResponsesProvider::new("https://api.openai.com/v1", Duration::from_secs(1))
                .is_ok()
        );
        assert!(
            OpenAiResponsesProvider::new("http://127.1.2.3:8080/v1", Duration::from_secs(1))
                .is_ok()
        );
        assert!(
            OpenAiResponsesProvider::new(
                "http://provider.localhost:8080/v1",
                Duration::from_secs(1)
            )
            .is_ok()
        );
        assert!(
            OpenAiResponsesProvider::new("http://[::1]:8080/v1", Duration::from_secs(1)).is_ok()
        );
        for invalid in [
            "http://example.com/v1",
            "ftp://example.com/v1",
            "https://user:secret@example.com/v1",
            "https://example.com/v1?key=secret",
            "https://example.com/v1#fragment",
        ] {
            assert!(
                OpenAiResponsesProvider::new(invalid, Duration::from_secs(1)).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn validates_bounded_request_input_before_network_access() {
        let mut oversized_model = request();
        oversized_model.model = "m".repeat(MAX_MODEL_ID_BYTES + 1);
        assert_eq!(
            validate_request(&oversized_model)
                .expect_err("oversized model")
                .code,
            CoreErrorCode::InvalidInput
        );

        let mut too_many_messages = request();
        too_many_messages.messages = (0..=MAX_PROMPT_MESSAGES)
            .map(|_| Message::user(too_many_messages.conversation_id.clone(), "x"))
            .collect();
        assert_eq!(
            validate_request(&too_many_messages)
                .expect_err("message count")
                .code,
            CoreErrorCode::InvalidInput
        );

        let mut oversized_prompt = request();
        oversized_prompt.messages = vec![Message::user(
            oversized_prompt.conversation_id.clone(),
            "x".repeat(MAX_PROMPT_BYTES + 1),
        )];
        assert_eq!(
            validate_request(&oversized_prompt)
                .expect_err("prompt size")
                .code,
            CoreErrorCode::InvalidInput
        );

        let mut cross_conversation = request();
        cross_conversation
            .messages
            .push(Message::user(ConversationId::new(), "wrong conversation"));
        assert_eq!(
            validate_request(&cross_conversation)
                .expect_err("cross-conversation prompt")
                .code,
            CoreErrorCode::InvalidInput
        );
    }

    #[tokio::test]
    async fn posts_responses_payload_with_bearer_auth_and_role_mapping() {
        let body = b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"usage\":{\"input_tokens\":7,\"output_tokens\":3}}}\n\n";
        let (base_url, capture) = stream_server(body, body.len());
        let provider =
            OpenAiResponsesProvider::new(&base_url, Duration::from_secs(2)).expect("provider");
        let (sink, _events) = mpsc::channel(4);
        let (_cancel, cancelled) = watch::channel(false);

        let usage = provider
            .generate(request(), Some(TEST_CREDENTIAL), sink, cancelled)
            .await
            .expect("valid completion");
        assert_eq!(
            usage,
            GenerationUsage {
                input_tokens: Some(7),
                output_tokens: Some(3),
                ..GenerationUsage::default()
            }
        );

        let captured = capture.recv().expect("captured request");
        assert!(captured.head.starts_with("POST /v1/responses HTTP/1.1\r\n"));
        assert!(
            captured
                .head
                .lines()
                .any(|line| line.eq_ignore_ascii_case(
                    "authorization: Bearer synthetic-test-token"
                ))
        );
        assert_eq!(captured.body["model"], "fixture-model");
        assert_eq!(captured.body["stream"], true);
        assert_eq!(captured.body["store"], false);
        assert_eq!(captured.body["temperature"], 0.25);
        assert_eq!(captured.body["max_output_tokens"], 321);
        assert_eq!(
            captured.body["input"],
            serde_json::json!([
                {"type":"message","role":"system","content":"system instruction"},
                {"type":"message","role":"user","content":"hello"},
                {"type":"message","role":"assistant","content":"prior answer"}
            ])
        );
    }

    #[tokio::test]
    async fn rejects_all_opaque_reasoning_continuity_before_network_access() {
        let canary = "unsupported-encrypted-canary";
        let provider =
            OpenAiResponsesProvider::new("http://127.0.0.1:9/v1", Duration::from_secs(1))
                .expect("provider");
        let mut preserve_requested = request();
        preserve_requested.preserve_opaque_reasoning_state = true;
        let mut prior_context = request();
        let source_message_id = prior_context
            .messages
            .last()
            .expect("assistant input")
            .id
            .clone();
        prior_context.opaque_reasoning_context = vec![OpaqueReasoningContext {
            source_message_id,
            api_family: ApiFamily::OpenAiResponses,
            model: prior_context.model.clone(),
            model_route_id: ModelRouteId::from("prior-route"),
            generation_preset_id: GenerationPresetId::from("prior-preset"),
            state: OpaqueReasoningState::OpenAiResponses {
                item: reasoning_item("reasoning-prior", canary),
            },
        }];

        for invalid in [preserve_requested, prior_context] {
            let (sink, _events) = mpsc::channel(4);
            let (_cancel, cancelled) = watch::channel(false);
            let error = provider
                .generate(invalid, Some(TEST_CREDENTIAL), sink, cancelled)
                .await
                .expect_err("opaque continuity must be unsupported");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert!(!error.message.contains(canary));
        }
    }

    #[tokio::test]
    async fn streams_fragmented_text_reasoning_summary_refusal_and_usage() {
        let body = concat!(
            ": keepalive\r\n\r\n",
            "event: response.reasoning_summary_text.delta\r\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"요약\"}\r\n\r\n",
            "data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"private raw reasoning\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"안녕\"}\n\n",
            "data: {\"type\":\"response.refusal.delta\",\"delta\":\"거절\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"usage\":{\"input_tokens\":11,\"input_tokens_details\":{\"cached_tokens\":7,\"cache_write_tokens\":4},\"output_tokens\":5,\"output_tokens_details\":{\"reasoning_tokens\":3},\"total_tokens\":16},\"output\":[{\"id\":\"reasoning-ignored\",\"type\":\"reasoning\",\"summary\":[],\"encrypted_content\":\"unsupported-output-canary\"}]}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"late\"}\n\n",
        );

        let (result, events) = generate_from_stream(body.as_bytes(), 3).await;

        let usage = result.expect("valid stream");
        assert_eq!(usage.input_tokens, Some(11));
        assert_eq!(usage.cached_read_tokens, Some(7));
        assert_eq!(usage.cached_write_tokens, Some(4));
        assert_eq!(usage.output_tokens, Some(5));
        assert_eq!(usage.reasoning_tokens, Some(3));
        assert_eq!(usage.tool_tokens, None);
        assert_eq!(
            usage
                .provider_raw_summary
                .as_ref()
                .map(lorepia_domain::BoundedJson::as_str),
            Some(r#"{"total_tokens":16}"#)
        );
        assert_eq!(
            events,
            vec![
                ProviderEvent::ReasoningDelta("요약".to_owned()),
                ProviderEvent::TextDelta("안녕".to_owned()),
                ProviderEvent::TextDelta("거절".to_owned()),
            ]
        );
        assert!(!format!("{events:?}").contains("unsupported-output-canary"));
    }

    #[tokio::test]
    async fn streams_exact_function_call_events_and_rejects_changed_final_arguments() {
        let (sink, mut received) = mpsc::channel(8);
        let (_cancel, mut cancelled) = watch::channel(false);
        let mut cancellation_open = true;
        let mut usage = GenerationUsage::default();
        let mut tool_calls = ResponsesToolCallTracker::default();
        let valid = [
            r#"data: {"type":"response.output_item.added","item":{"id":"item-1","type":"function_call","call_id":"call-1","name":"lorepia_probe","arguments":""}}"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"item-1","delta":"{\"probe\":"}"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"item-1","delta":"\"ok\"}"}"#,
            r#"data: {"type":"response.function_call_arguments.done","item_id":"item-1","name":"lorepia_probe","arguments":"{\"probe\":\"ok\"}"}"#,
            r#"data: {"type":"response.completed","response":{"status":"completed","error":null,"usage":{"input_tokens":8,"output_tokens":3}}}"#,
        ];
        for event in valid {
            process_event(
                event.as_bytes(),
                &sink,
                &mut usage,
                &mut tool_calls,
                &mut cancelled,
                &mut cancellation_open,
            )
            .await
            .expect("valid function-call event");
        }
        let mut events = Vec::new();
        while let Ok(event) = received.try_recv() {
            events.push(event);
        }
        let id = ToolCallId::parse("call-1").expect("tool call ID");
        assert_eq!(
            events,
            vec![
                ProviderEvent::ToolCallStarted {
                    id: id.clone(),
                    name: ToolName::parse("lorepia_probe").expect("tool name"),
                },
                ProviderEvent::ToolCallArgumentsDelta {
                    id: id.clone(),
                    delta: ToolCallArgumentsDelta::parse(r#"{"probe":"#).expect("first delta"),
                },
                ProviderEvent::ToolCallArgumentsDelta {
                    id: id.clone(),
                    delta: ToolCallArgumentsDelta::parse(r#""ok"}"#).expect("second delta"),
                },
                ProviderEvent::ToolCallCompleted { id },
            ]
        );

        let (sink, _received) = mpsc::channel(8);
        let (_cancel, mut cancelled) = watch::channel(false);
        let mut cancellation_open = true;
        let mut usage = GenerationUsage::default();
        let mut tool_calls = ResponsesToolCallTracker::default();
        for event in [
            r#"data: {"type":"response.output_item.added","item":{"id":"item-1","type":"function_call","call_id":"call-1","name":"lorepia_probe"}}"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"item-1","delta":"{\"probe\":\"ok\"}"}"#,
        ] {
            process_event(
                event.as_bytes(),
                &sink,
                &mut usage,
                &mut tool_calls,
                &mut cancelled,
                &mut cancellation_open,
            )
            .await
            .expect("valid partial function-call event");
        }
        let result = process_event(
            br#"data: {"type":"response.function_call_arguments.done","item_id":"item-1","name":"lorepia_probe","arguments":"{\"probe\":\"changed\"}"}"#,
            &sink,
            &mut usage,
            &mut tool_calls,
            &mut cancelled,
            &mut cancellation_open,
        )
        .await;
        assert_eq!(
            result.expect_err("changed arguments must fail").code,
            CoreErrorCode::ProviderUnavailable
        );
    }

    #[tokio::test]
    async fn requires_credential_before_network_access() {
        let provider =
            OpenAiResponsesProvider::new("http://127.0.0.1:9/v1", Duration::from_secs(1))
                .expect("provider");
        let (sink, _events) = mpsc::channel(4);
        let (_cancel, cancelled) = watch::channel(false);

        let error = provider
            .generate(request(), None, sink, cancelled)
            .await
            .expect_err("credential is mandatory");

        assert_eq!(error.code, CoreErrorCode::ProviderAuthFailed);
        assert_eq!(error.message, "provider credential is required");
        assert!(!error.recoverable);
    }

    #[tokio::test]
    async fn maps_auth_rate_limit_and_redirect_without_following() {
        let auth = status_error_from("401 Unauthorized", "").await;
        assert_eq!(auth.code, CoreErrorCode::ProviderAuthFailed);
        assert!(!auth.recoverable);

        let rate_limit = status_error_from("429 Too Many Requests", "").await;
        assert_eq!(rate_limit.code, CoreErrorCode::ProviderRateLimited);
        assert!(rate_limit.recoverable);

        let redirect = status_error_from(
            "302 Found",
            "Location: https://example.invalid/redirect\r\n",
        )
        .await;
        assert_eq!(redirect.code, CoreErrorCode::ProviderUnavailable);
        assert!(!redirect.recoverable);

        let bad_request = status_error(StatusCode::BAD_REQUEST);
        assert_eq!(bad_request.code, CoreErrorCode::InvalidInput);
        assert!(!bad_request.recoverable);
    }

    #[tokio::test]
    async fn rejects_non_event_stream_success_response() {
        let body = b"{\"id\":\"response\"}";
        let (base_url, _capture) =
            stream_server_with_content_type(body, body.len(), "application/json");
        let provider =
            OpenAiResponsesProvider::new(&base_url, Duration::from_secs(2)).expect("provider");
        let (sink, _events) = mpsc::channel(4);
        let (_cancel, cancelled) = watch::channel(false);

        let error = provider
            .generate(request(), Some(TEST_CREDENTIAL), sink, cancelled)
            .await
            .expect_err("content type must be event-stream");

        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(error.message, "provider returned a non-streaming response");
    }

    #[tokio::test]
    async fn rejects_failed_malformed_and_unterminated_streams_without_leaking_details() {
        let cases = [
            (
                b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"sensitive-provider-detail\"}}}\n\n"
                    .as_slice(),
                "provider reported a streaming failure",
            ),
            (
                b"data: {not-json}\n\n".as_slice(),
                "provider returned malformed streaming data",
            ),
            (
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n"
                    .as_slice(),
                "provider stream ended before response.completed",
            ),
            (
                b"data: [DONE]\n\n".as_slice(),
                "provider stream ended before response.completed",
            ),
        ];

        for (body, expected_message) in cases {
            let (result, _events) = generate_from_stream(body, 2).await;
            let error = result.expect_err("invalid stream must fail");
            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
            assert_eq!(error.message, expected_message);
            assert!(!error.message.contains("sensitive-provider-detail"));
            assert!(!error.message.contains(TEST_CREDENTIAL));
        }
    }

    #[tokio::test]
    async fn cancellation_interrupts_sink_backpressure() {
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"first\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"second\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"usage\":null}}\n\n",
        );
        let (base_url, _capture) = stream_server(body.as_bytes(), body.len());
        let provider =
            OpenAiResponsesProvider::new(&base_url, Duration::from_secs(2)).expect("provider");
        let (sink, mut events) = mpsc::channel(1);
        let (cancel, cancelled) = watch::channel(false);
        let generation = tokio::spawn(async move {
            provider
                .generate(request(), Some(TEST_CREDENTIAL), sink, cancelled)
                .await
        });

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
            .expect_err("generation must be cancelled");
        assert_eq!(error.code, CoreErrorCode::Cancelled);
        assert_eq!(
            events.try_recv().expect("first event"),
            ProviderEvent::TextDelta("first".to_owned())
        );
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn observes_cancellation_before_auth_or_network() {
        let provider =
            OpenAiResponsesProvider::new("http://127.0.0.1:9/v1", Duration::from_secs(1))
                .expect("provider");
        let (sink, _events) = mpsc::channel(4);
        let (cancel, cancelled) = watch::channel(false);
        cancel.send(true).expect("cancel");

        let error = provider
            .generate(request(), None, sink, cancelled)
            .await
            .expect_err("generation must be cancelled");

        assert_eq!(error.code, CoreErrorCode::Cancelled);
    }

    #[test]
    fn enforces_event_and_total_stream_bounds() {
        assert!(ensure_event_size(MAX_SSE_EVENT_BYTES).is_ok());
        assert_eq!(
            ensure_event_size(MAX_SSE_EVENT_BYTES + 1)
                .expect_err("oversized event")
                .message,
            "provider streaming event exceeded 1 MiB"
        );
        assert!(ensure_stream_size(MAX_STREAM_BYTES).is_ok());
        assert_eq!(
            ensure_stream_size(MAX_STREAM_BYTES + 1)
                .expect_err("oversized stream")
                .message,
            "provider stream exceeded 64 MiB"
        );
    }
}
