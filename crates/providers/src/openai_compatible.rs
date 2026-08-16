use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    time::Duration,
};

use async_trait::async_trait;
use futures_util::StreamExt;
use lorepia_domain::{
    ApiFamily, AuthBinding, CoreError, CoreErrorCode, CoreResult, GenerationRequest,
    GenerationUsage, MAX_OPAQUE_REASONING_ITEM_BYTES, MAX_OPAQUE_REASONING_STATE_COUNT,
    MAX_OPAQUE_REASONING_TOTAL_BYTES, MAX_TOOL_CALL_ID_BYTES, MAX_TOOL_NAME_BYTES, MessageRole,
    OpaqueReasoningState, OpenRouterReasoningDetail, OpenRouterReasoningTopology,
    ProviderCapabilities, ToolCallArgumentsDelta, ToolCallId, ToolName,
    validate_opaque_reasoning_states,
};
use reqwest::{StatusCode, header::CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use url::Url;
use zeroize::Zeroize;

use crate::{
    Provider, ProviderEvent, ProviderEventSender, merge_usage_summary,
    network_transport::{ProviderHttpTarget, authorize_request, validate_credential_for_auth},
    parameter_mapping::{PromptCacheWireDialect, ProviderRequestPlan},
    prompt_contract::ProviderPromptAdapterContract,
    request_plan::planned_json_payload,
};

const MAX_SSE_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
const MAX_PENDING_TOOL_ARGUMENT_BYTES: usize = 256 * 1024;
const MAX_PROMPT_MESSAGES: usize = 128;
const MAX_PROMPT_BYTES: usize = 512 * 1024;
const MAX_PROMPT_CHARS: usize = 128 * 1024;
const MAX_MODEL_ID_BYTES: usize = 1024;
const MAX_MODEL_ID_CHARS: usize = 256;
const LEGACY_FUNCTION_CALL_INDEX: u32 = u32::MAX;
const LEGACY_FUNCTION_CALL_ID: &str = "legacy-function-call-0";
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

#[derive(Clone)]
pub struct OpenAiCompatibleProvider {
    endpoint: Url,
    target: ProviderHttpTarget,
    manifest_auth: Option<AuthBinding>,
    request_plan: Option<ProviderRequestPlan>,
    dialect: OpenAiCompatibleDialect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAiCompatibleDialect {
    Standard,
    OpenRouter,
}

impl OpenAiCompatibleProvider {
    pub fn new(base_url: &str, timeout: Duration) -> CoreResult<Self> {
        let mut endpoint = Url::parse(base_url)
            .map_err(|error| CoreError::invalid(format!("invalid provider base URL: {error}")))?;
        validate_endpoint(&endpoint)?;
        if !endpoint.path().ends_with('/') {
            let path = format!("{}/", endpoint.path());
            endpoint.set_path(&path);
        }
        endpoint = endpoint.join("chat/completions").map_err(|error| {
            CoreError::invalid(format!("cannot construct provider endpoint: {error}"))
        })?;
        let target = ProviderHttpTarget::inferred(endpoint.as_str(), timeout)?;
        Ok(Self {
            endpoint,
            target,
            manifest_auth: None,
            request_plan: None,
            dialect: OpenAiCompatibleDialect::Standard,
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
            manifest_auth: Some(auth),
            request_plan: None,
            dialect: OpenAiCompatibleDialect::Standard,
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

    pub(crate) fn with_openrouter_reasoning_details(mut self, enabled: bool) -> Self {
        self.dialect = if enabled {
            OpenAiCompatibleDialect::OpenRouter
        } else {
            OpenAiCompatibleDialect::Standard
        };
        self
    }
}

#[async_trait]
impl Provider for OpenAiCompatibleProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: true,
            max_context_tokens: None,
        }
    }

    fn snapshot_request(&self, request: &GenerationRequest) -> CoreResult<serde_json::Value> {
        crate::validate_snapshot_request(request)?;
        validate_request(request, self.dialect)?;
        let cache_dialect = self
            .request_plan
            .as_ref()
            .map_or(PromptCacheWireDialect::Unsupported, |plan| {
                plan.cache_dialect()
            });
        let payload = request_payload_with_cache(request.clone(), self.dialect, cache_dialect)?;
        planned_json_payload(
            &payload,
            ApiFamily::OpenAiChatCompletions,
            self.request_plan.as_ref(),
        )
    }

    #[allow(clippy::too_many_lines)]
    async fn generate(
        &self,
        request: GenerationRequest,
        credential: Option<&str>,
        sink: ProviderEventSender,
        mut cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        if *cancelled.borrow() {
            return Err(cancelled_error());
        }
        validate_request(&request, self.dialect)?;
        let preserve_opaque_reasoning_state = request.preserve_opaque_reasoning_state;
        let cache_dialect = self
            .request_plan
            .as_ref()
            .map_or(PromptCacheWireDialect::Unsupported, |plan| {
                plan.cache_dialect()
            });
        let payload = request_payload_with_cache(request, self.dialect, cache_dialect)?;
        let payload = planned_json_payload(
            &payload,
            ApiFamily::OpenAiChatCompletions,
            self.request_plan.as_ref(),
        )?;
        if let Some(auth) = &self.manifest_auth {
            validate_credential_for_auth(auth, credential)?;
        }
        let prepared = self.target.prepare().await?;
        ensure_not_cancelled(&cancelled)?;
        let mut builder = prepared.client().post(self.endpoint.clone()).json(&payload);
        if let Some(auth) = &self.manifest_auth {
            builder = authorize_request(builder, auth, credential)?;
        } else if let Some(credential) = credential.filter(|value| !value.is_empty()) {
            builder = builder.bearer_auth(credential);
        }
        let response = builder.send();
        tokio::pin!(response);
        let mut cancellation_open = true;
        let response = loop {
            tokio::select! {
                change = cancelled.changed(), if cancellation_open => {
                    if change.is_ok() && *cancelled.borrow() {
                        return Err(cancelled_error());
                    }
                    if change.is_err() {
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

        let mut bytes = response.bytes_stream();
        let mut pending = Vec::<u8>::new();
        let mut total_stream_bytes = 0_usize;
        let mut usage = GenerationUsage::default();
        let mut stream_state = SseStreamState::default();
        let mut tool_calls = OpenAiToolCallTracker::default();
        let mut reasoning_details = OpenRouterReasoningTracker::default();
        loop {
            tokio::select! {
                change = cancelled.changed(), if cancellation_open => {
                    if change.is_ok() && *cancelled.borrow() {
                        return Err(cancelled_error());
                    }
                    if change.is_err() {
                        cancellation_open = false;
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
                            ensure_not_cancelled(&cancelled)?;
                            ensure_event_size(boundary)?;
                            let event = pending.drain(..boundary).collect::<Vec<_>>();
                            pending.drain(..separator_len);
                            if process_event(
                                &event,
                                &sink,
                                &mut usage,
                                &mut stream_state,
                                &mut tool_calls,
                                &mut reasoning_details,
                                self.dialect,
                                preserve_opaque_reasoning_state,
                                &mut cancelled,
                                &mut cancellation_open,
                            )
                            .await?
                                == EventAction::Done
                            {
                                ensure_not_cancelled(&cancelled)?;
                                // `[DONE]` is terminal. Stop consuming immediately so bytes sent
                                // after the marker can never be forwarded as provider events.
                                return Ok(usage);
                            }
                        }
                    }
                    ensure_pending_size(&pending, false)?;
                }
            }
        }
        while let Some((boundary, separator_len)) = find_event_boundary(&pending, true) {
            ensure_not_cancelled(&cancelled)?;
            ensure_event_size(boundary)?;
            let event = pending.drain(..boundary).collect::<Vec<_>>();
            pending.drain(..separator_len);
            if process_event(
                &event,
                &sink,
                &mut usage,
                &mut stream_state,
                &mut tool_calls,
                &mut reasoning_details,
                self.dialect,
                preserve_opaque_reasoning_state,
                &mut cancelled,
                &mut cancellation_open,
            )
            .await?
                == EventAction::Done
            {
                ensure_not_cancelled(&cancelled)?;
                return Ok(usage);
            }
        }
        ensure_pending_size(&pending, true)?;
        if !pending.is_empty() {
            return Err(streaming_error(
                "provider stream ended with an incomplete event",
            ));
        }
        Err(stream_state.incomplete_error())
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

#[cfg(test)]
fn request_payload(
    request: GenerationRequest,
    dialect: OpenAiCompatibleDialect,
) -> CoreResult<RequestPayload> {
    request_payload_with_cache(request, dialect, PromptCacheWireDialect::Unsupported)
}

// Keep resolved and legacy OpenRouter topology replay in one audited
// function so their one-to-one opaque-state checks cannot drift apart.
#[allow(clippy::too_many_lines)]
fn request_payload_with_cache(
    request: GenerationRequest,
    dialect: OpenAiCompatibleDialect,
    cache_dialect: PromptCacheWireDialect,
) -> CoreResult<RequestPayload> {
    if let Some(plan) = &request.resolved_prompt_plan {
        let compiled = prompt_contract()
            .compile_resolved_plan_for_execution(plan, cache_dialect)
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        compiled
            .verify_execution_hash(request.provider_execution_plan_hash.as_deref())
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        let mut used_context_messages = BTreeSet::new();
        let mut messages = Vec::with_capacity(compiled.messages().len());
        for message in compiled.messages() {
            let (reasoning, reasoning_details) = if dialect == OpenAiCompatibleDialect::OpenRouter
                && request.preserve_opaque_reasoning_state
                && message.effective_role() == lorepia_domain::ProviderMessageRole::Assistant
            {
                let mut matching = request.opaque_reasoning_context.iter().filter(|context| {
                    message
                        .source_message_ids()
                        .contains(&context.source_message_id)
                });
                let Some(context) = matching.next() else {
                    messages.push(RequestMessage {
                        role: message.wire_role().as_str(),
                        content: message.content().to_owned(),
                        reasoning: None,
                        reasoning_details: None,
                    });
                    continue;
                };
                if matching.next().is_some() {
                    return Err(CoreError::invalid(
                        "opaque reasoning state repeated an OpenRouter message topology",
                    ));
                }
                let OpaqueReasoningState::OpenRouterReasoning { topology } = &context.state else {
                    return Err(CoreError::invalid(
                        "opaque reasoning state format does not match OpenRouter",
                    ));
                };
                if !used_context_messages.insert(context.source_message_id.0.clone()) {
                    return Err(CoreError::invalid(
                        "opaque reasoning state was reused by multiple resolved prompt messages",
                    ));
                }
                let details = topology
                    .reasoning_details()
                    .map(|details| {
                        details
                            .iter()
                            .map(|detail| {
                                serde_json::from_str(detail.expose_to_provider()).map_err(|_| {
                                    CoreError::invalid(
                                        "stored OpenRouter reasoning detail is malformed",
                                    )
                                })
                            })
                            .collect::<CoreResult<Vec<_>>>()
                    })
                    .transpose()?;
                (topology.reasoning().map(str::to_owned), details)
            } else {
                (None, None)
            };
            messages.push(RequestMessage {
                role: message.wire_role().as_str(),
                content: message.content().to_owned(),
                reasoning,
                reasoning_details,
            });
        }
        if request.preserve_opaque_reasoning_state
            && request
                .opaque_reasoning_context
                .iter()
                .any(|context| !used_context_messages.contains(&context.source_message_id.0))
        {
            return Err(CoreError::invalid(
                "opaque reasoning state source message is not an assistant input",
            ));
        }
        return Ok(RequestPayload {
            model: request.model,
            messages,
            stream: true,
            temperature: request.temperature,
            max_tokens: request.max_output_tokens,
            stream_options: (dialect == OpenAiCompatibleDialect::OpenRouter).then_some(
                StreamOptions {
                    include_usage: true,
                },
            ),
        });
    }

    let mut used_context_messages = BTreeSet::new();
    let mut messages = Vec::with_capacity(request.messages.len());
    for message in request.messages {
        let (reasoning, reasoning_details) = if dialect == OpenAiCompatibleDialect::OpenRouter
            && request.preserve_opaque_reasoning_state
            && message.role == MessageRole::Assistant
        {
            let mut matching = request
                .opaque_reasoning_context
                .iter()
                .filter(|context| context.source_message_id == message.id);
            let Some(context) = matching.next() else {
                messages.push(RequestMessage {
                    role: role_name(message.role),
                    content: message.content,
                    reasoning: None,
                    reasoning_details: None,
                });
                continue;
            };
            if matching.next().is_some() {
                return Err(CoreError::invalid(
                    "opaque reasoning state repeated an OpenRouter message topology",
                ));
            }
            let OpaqueReasoningState::OpenRouterReasoning { topology } = &context.state else {
                return Err(CoreError::invalid(
                    "opaque reasoning state format does not match OpenRouter",
                ));
            };
            if !used_context_messages.insert(message.id.0.clone()) {
                return Err(CoreError::invalid(
                    "opaque reasoning state was reused by multiple prompt messages",
                ));
            }
            let details = topology
                .reasoning_details()
                .map(|details| {
                    details
                        .iter()
                        .map(|detail| {
                            serde_json::from_str(detail.expose_to_provider()).map_err(|_| {
                                CoreError::invalid(
                                    "stored OpenRouter reasoning detail is malformed",
                                )
                            })
                        })
                        .collect::<CoreResult<Vec<_>>>()
                })
                .transpose()?;
            (topology.reasoning().map(str::to_owned), details)
        } else {
            (None, None)
        };
        messages.push(RequestMessage {
            role: role_name(message.role),
            content: message.content,
            reasoning,
            reasoning_details,
        });
    }
    if request.preserve_opaque_reasoning_state
        && request
            .opaque_reasoning_context
            .iter()
            .any(|context| !used_context_messages.contains(&context.source_message_id.0))
    {
        return Err(CoreError::invalid(
            "opaque reasoning state source message is not an assistant input",
        ));
    }
    Ok(RequestPayload {
        model: request.model,
        messages,
        stream: true,
        temperature: request.temperature,
        max_tokens: request.max_output_tokens,
        stream_options: (dialect == OpenAiCompatibleDialect::OpenRouter).then_some(StreamOptions {
            include_usage: true,
        }),
    })
}

fn validate_request(
    request: &GenerationRequest,
    dialect: OpenAiCompatibleDialect,
) -> CoreResult<()> {
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
    validate_opaque_request_context(request, dialect)
}

fn validate_opaque_request_context(
    request: &GenerationRequest,
    dialect: OpenAiCompatibleDialect,
) -> CoreResult<()> {
    if dialect != OpenAiCompatibleDialect::OpenRouter {
        if !request.preserve_opaque_reasoning_state && request.opaque_reasoning_context.is_empty() {
            return Ok(());
        }
        return Err(CoreError::invalid(
            "this OpenAI-compatible route cannot preserve opaque reasoning state",
        ));
    }
    if !request.preserve_opaque_reasoning_state {
        if request.opaque_reasoning_context.is_empty() {
            return Ok(());
        }
        return Err(CoreError::invalid(
            "opaque reasoning state requires continuity preservation",
        ));
    }
    let provenance = request
        .provider_provenance
        .as_ref()
        .ok_or_else(|| CoreError::invalid("opaque reasoning state requires provider provenance"))?;
    if provenance.api_family != ApiFamily::OpenAiChatCompletions {
        return Err(CoreError::invalid(
            "opaque reasoning state API family does not match OpenRouter",
        ));
    }
    let states = request
        .opaque_reasoning_context
        .iter()
        .map(|context| {
            if context.api_family != ApiFamily::OpenAiChatCompletions
                || context.model != request.model
                || context.model_route_id != provenance.model_route_id
            {
                return Err(CoreError::invalid(
                    "opaque reasoning state provenance does not match the current provider route",
                ));
            }
            if !matches!(
                context.state,
                OpaqueReasoningState::OpenRouterReasoning { .. }
            ) {
                return Err(CoreError::invalid(
                    "opaque reasoning state format does not match OpenRouter",
                ));
            }
            Ok(context.state.clone())
        })
        .collect::<CoreResult<Vec<_>>>()?;
    validate_opaque_reasoning_states(&states).map_err(CoreError::invalid)
}

fn prompt_too_large_error() -> CoreError {
    CoreError::invalid("provider prompt exceeded the input safety limit")
}

fn cancelled_error() -> CoreError {
    CoreError::new(CoreErrorCode::Cancelled, "generation was cancelled", true)
}

fn ensure_not_cancelled(cancelled: &watch::Receiver<bool>) -> CoreResult<()> {
    if *cancelled.borrow() {
        Err(cancelled_error())
    } else {
        Ok(())
    }
}

#[derive(Serialize)]
struct RequestPayload {
    model: String,
    messages: Vec<RequestMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Serialize)]
struct RequestMessage {
    role: &'static str,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_details: Option<Vec<serde_json::Value>>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    usage: Option<StreamUsage>,
}

#[derive(Deserialize)]
struct StreamChoice {
    index: u32,
    delta: StreamDelta,
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct StreamDelta {
    content: Option<String>,
    reasoning: Option<String>,
    reasoning_content: Option<String>,
    reasoning_details: Option<serde_json::Value>,
    #[serde(default)]
    tool_calls: Vec<OpenAiToolCallDelta>,
    function_call: Option<OpenAiFunctionCallDelta>,
}

#[derive(Deserialize)]
struct OpenAiToolCallDelta {
    index: Option<u32>,
    id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    function: Option<OpenAiFunctionCallDelta>,
}

#[derive(Default, Deserialize)]
struct OpenAiFunctionCallDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Default)]
struct OpenAiToolCallTracker {
    calls: BTreeMap<u32, PartialOpenAiToolCall>,
    started_ids: BTreeSet<ToolCallId>,
}

#[derive(Default)]
struct PartialOpenAiToolCall {
    id: String,
    name: String,
    pending_arguments: Vec<ToolCallArgumentsDelta>,
    pending_argument_bytes: usize,
    started: Option<ToolCallId>,
}

#[derive(Default)]
struct OpenRouterReasoningTracker {
    reasoning: Option<String>,
    reasoning_details_observed: bool,
    details: Vec<serde_json::Value>,
}

impl OpenRouterReasoningTracker {
    fn observe(
        &mut self,
        reasoning: Option<&str>,
        reasoning_details: Option<&serde_json::Value>,
        enabled: bool,
    ) -> CoreResult<()> {
        if !enabled {
            return Ok(());
        }
        if let Some(reasoning) = reasoning {
            let accumulated = self.reasoning.get_or_insert_with(String::new);
            accumulated.push_str(reasoning);
            if accumulated.len() > MAX_OPAQUE_REASONING_TOTAL_BYTES {
                self.wipe();
                return Err(streaming_error(
                    "provider returned oversized OpenRouter reasoning",
                ));
            }
            self.validate_detail_bounds()?;
        }
        let Some(value) = reasoning_details else {
            return Ok(());
        };
        self.reasoning_details_observed = true;
        let Some(details) = value.as_array() else {
            self.wipe();
            return Err(streaming_error(
                "provider returned malformed OpenRouter reasoning details",
            ));
        };
        if details.len() > MAX_OPAQUE_REASONING_STATE_COUNT {
            self.wipe();
            return Err(streaming_error(
                "provider returned too many OpenRouter reasoning fragments",
            ));
        }
        for detail in details {
            if let Err(error) = self.observe_detail(detail) {
                self.wipe();
                return Err(error);
            }
        }
        Ok(())
    }

    fn finish(&mut self, has_tool_calls: bool) -> CoreResult<Vec<ProviderEvent>> {
        let observed = self.reasoning.is_some() || self.reasoning_details_observed;
        if has_tool_calls && observed {
            self.wipe();
            return Err(streaming_error(
                "provider returned reasoning continuity with unsupported tool topology",
            ));
        }
        if !observed {
            return Ok(Vec::new());
        }
        let typed_details = self
            .details
            .iter()
            .map(|detail| {
                OpenRouterReasoningDetail::from_value(detail).map_err(|_| {
                    streaming_error("provider returned incomplete OpenRouter reasoning details")
                })
            })
            .collect::<CoreResult<Vec<_>>>();
        let typed_details = match typed_details {
            Ok(details) => details,
            Err(error) => {
                self.wipe();
                return Err(error);
            }
        };
        let topology = OpenRouterReasoningTopology::new(
            self.reasoning.take(),
            self.reasoning_details_observed.then_some(typed_details),
        );
        let Ok(topology) = topology else {
            self.wipe();
            return Err(streaming_error(
                "provider returned invalid OpenRouter reasoning topology",
            ));
        };
        self.reasoning_details_observed = false;
        wipe_json_strings(&mut self.details);
        self.details.clear();
        Ok(vec![ProviderEvent::OpaqueReasoningState(
            OpaqueReasoningState::OpenRouterReasoning { topology },
        )])
    }

    fn observe_detail(&mut self, detail: &serde_json::Value) -> CoreResult<()> {
        validate_openrouter_reasoning_fragment(detail)?;
        let kind = detail
            .get("type")
            .and_then(serde_json::Value::as_str)
            .expect("fragment validation requires type");
        let mergeable = matches!(kind, "reasoning.text" | "reasoning.summary");
        if mergeable
            && let Some(previous) = self.details.last_mut()
            && can_merge_openrouter_fragments(previous, detail)?
        {
            merge_openrouter_fragment(previous, detail)?;
            return self.validate_detail_bounds();
        }
        self.details.push(detail.clone());
        self.validate_detail_bounds()
    }

    fn validate_detail_bounds(&mut self) -> CoreResult<()> {
        if self.details.len() > MAX_OPAQUE_REASONING_STATE_COUNT {
            self.wipe();
            return Err(streaming_error(
                "provider returned too many OpenRouter reasoning details",
            ));
        }
        let initial = self.reasoning.as_ref().map_or(0, String::len);
        let validation = self.details.iter().try_fold(initial, |total, detail| {
            let mut bytes = serde_json::to_vec(detail)
                .map_err(|_| streaming_error("provider returned malformed reasoning details"))?;
            let byte_len = bytes.len();
            bytes.zeroize();
            if byte_len > MAX_OPAQUE_REASONING_ITEM_BYTES {
                return Err(streaming_error(
                    "provider returned oversized OpenRouter reasoning details",
                ));
            }
            total.checked_add(byte_len).ok_or_else(|| {
                streaming_error("provider returned oversized OpenRouter reasoning details")
            })
        });
        if validation.is_err()
            || validation.is_ok_and(|total| total > MAX_OPAQUE_REASONING_TOTAL_BYTES)
        {
            self.wipe();
            return Err(streaming_error(
                "provider returned oversized OpenRouter reasoning details",
            ));
        }
        Ok(())
    }

    fn wipe(&mut self) {
        if let Some(reasoning) = &mut self.reasoning {
            reasoning.zeroize();
        }
        self.reasoning = None;
        self.reasoning_details_observed = false;
        wipe_json_strings(&mut self.details);
        self.details.clear();
    }
}

impl Drop for OpenRouterReasoningTracker {
    fn drop(&mut self) {
        self.wipe();
    }
}

fn validate_openrouter_reasoning_fragment(detail: &serde_json::Value) -> CoreResult<()> {
    let object = detail.as_object().ok_or_else(|| {
        streaming_error("provider returned malformed OpenRouter reasoning details")
    })?;
    let kind = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            streaming_error("provider returned malformed OpenRouter reasoning details")
        })?;
    if !matches!(
        kind,
        "reasoning.text" | "reasoning.summary" | "reasoning.encrypted"
    ) {
        return Err(streaming_error(
            "provider returned unsupported OpenRouter reasoning details",
        ));
    }
    for field in ["id", "format"] {
        if object
            .get(field)
            .is_some_and(|value| !value.is_null() && !value.is_string())
        {
            return Err(streaming_error(
                "provider returned malformed OpenRouter reasoning metadata",
            ));
        }
    }
    if object
        .get("index")
        .is_some_and(|value| !value.is_null() && value.as_u64().is_none())
    {
        return Err(streaming_error(
            "provider returned malformed OpenRouter reasoning index",
        ));
    }
    let payload = match kind {
        "reasoning.text" => "text",
        "reasoning.summary" => "summary",
        "reasoning.encrypted" => "data",
        _ => unreachable!("closed kind"),
    };
    if object
        .get(payload)
        .is_some_and(|value| !value.is_null() && !value.is_string())
        || (kind == "reasoning.text"
            && object
                .get("signature")
                .is_some_and(|value| !value.is_null() && !value.is_string()))
    {
        return Err(streaming_error(
            "provider returned malformed OpenRouter reasoning payload",
        ));
    }
    Ok(())
}

fn can_merge_openrouter_fragments(
    previous: &serde_json::Value,
    incoming: &serde_json::Value,
) -> CoreResult<bool> {
    let previous = previous
        .as_object()
        .expect("stored reasoning fragments are objects");
    let incoming = incoming
        .as_object()
        .expect("validated reasoning fragments are objects");
    if previous.get("type") != incoming.get("type") {
        return Ok(false);
    }
    let previous_index = previous.get("index").and_then(serde_json::Value::as_u64);
    let incoming_index = incoming.get("index").and_then(serde_json::Value::as_u64);
    let previous_id = previous.get("id").filter(|value| !value.is_null());
    let incoming_id = incoming.get("id").filter(|value| !value.is_null());
    if let (Some(left), Some(right)) = (previous_index, incoming_index) {
        if left != right {
            return Err(streaming_error(
                "provider returned conflicting OpenRouter reasoning metadata",
            ));
        }
        if previous_id.is_some() && incoming_id.is_some() && previous_id != incoming_id {
            return Err(streaming_error(
                "provider returned conflicting OpenRouter reasoning metadata",
            ));
        }
    } else if previous_id.is_some() && incoming_id.is_some() && previous_id != incoming_id {
        return Err(streaming_error(
            "provider returned conflicting OpenRouter reasoning metadata",
        ));
    }
    for field in ["format", "signature"] {
        if let (Some(left), Some(right)) = (
            previous.get(field).filter(|value| !value.is_null()),
            incoming.get(field).filter(|value| !value.is_null()),
        ) && left != right
        {
            return Err(streaming_error(
                "provider returned conflicting OpenRouter reasoning metadata",
            ));
        }
    }
    Ok(true)
}

fn merge_openrouter_fragment(
    previous: &mut serde_json::Value,
    incoming: &serde_json::Value,
) -> CoreResult<()> {
    let previous = previous
        .as_object_mut()
        .expect("stored reasoning fragments are objects");
    let incoming = incoming
        .as_object()
        .expect("validated reasoning fragments are objects");
    let kind = incoming
        .get("type")
        .and_then(serde_json::Value::as_str)
        .expect("validated reasoning type");
    let payload = if kind == "reasoning.text" {
        "text"
    } else {
        "summary"
    };
    if let Some(fragment) = incoming.get(payload).and_then(serde_json::Value::as_str) {
        let value = previous
            .entry(payload.to_owned())
            .or_insert_with(|| serde_json::Value::String(String::new()));
        match value {
            serde_json::Value::String(value) => value.push_str(fragment),
            serde_json::Value::Null => {
                *value = serde_json::Value::String(fragment.to_owned());
            }
            _ => {
                return Err(streaming_error(
                    "provider returned conflicting OpenRouter reasoning payload",
                ));
            }
        }
    }
    for (field, value) in incoming {
        if matches!(field.as_str(), "type" | "text" | "summary") || value.is_null() {
            continue;
        }
        match previous.get(field) {
            Some(existing) if !existing.is_null() && existing != value => {
                return Err(streaming_error(
                    "provider returned conflicting OpenRouter reasoning metadata",
                ));
            }
            Some(existing) if !existing.is_null() => {}
            _ => {
                previous.insert(field.clone(), value.clone());
            }
        }
    }
    Ok(())
}

fn wipe_json_strings(values: &mut [serde_json::Value]) {
    fn wipe(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(value) => value.zeroize(),
            serde_json::Value::Array(values) => values.iter_mut().for_each(wipe),
            serde_json::Value::Object(object) => {
                for (mut key, mut value) in std::mem::take(object) {
                    key.zeroize();
                    wipe(&mut value);
                }
            }
            serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            }
        }
    }
    values.iter_mut().for_each(wipe);
}

impl StreamDelta {
    fn visible_reasoning(&self) -> CoreResult<Option<&str>> {
        match (self.reasoning.as_deref(), self.reasoning_content.as_deref()) {
            (Some(reasoning), Some(alias)) if reasoning != alias => Err(streaming_error(
                "provider returned conflicting reasoning fields",
            )),
            (Some(reasoning), _) => Ok(Some(reasoning)),
            (None, alias) => Ok(alias),
        }
    }
}

#[derive(Deserialize)]
struct StreamUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
    prompt_tokens_details: Option<PromptTokensDetails>,
    completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)]
struct PromptTokensDetails {
    cached_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    audio_tokens: Option<u64>,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)]
struct CompletionTokensDetails {
    reasoning_tokens: Option<u64>,
    audio_tokens: Option<u64>,
    accepted_prediction_tokens: Option<u64>,
    rejected_prediction_tokens: Option<u64>,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum SseStreamState {
    #[default]
    AwaitingData,
    Streaming,
    Terminal,
}

impl SseStreamState {
    fn incomplete_error(&self) -> CoreError {
        match self {
            Self::AwaitingData => streaming_error("provider returned an empty streaming response"),
            Self::Streaming | Self::Terminal => {
                streaming_error("provider stream ended before [DONE]")
            }
        }
    }

    fn observe_payload(&mut self) {
        if *self == Self::AwaitingData {
            *self = Self::Streaming;
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum EventAction {
    Continue,
    Done,
}

#[allow(clippy::too_many_arguments)]
async fn process_event(
    event: &[u8],
    sink: &ProviderEventSender,
    usage: &mut GenerationUsage,
    state: &mut SseStreamState,
    tool_calls: &mut OpenAiToolCallTracker,
    reasoning_details: &mut OpenRouterReasoningTracker,
    dialect: OpenAiCompatibleDialect,
    preserve_opaque_reasoning_state: bool,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<EventAction> {
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
        return Ok(EventAction::Continue);
    }

    let data = data_lines.join("\n");
    let data = data.trim();
    if data.is_empty() {
        return Ok(EventAction::Continue);
    }
    if data == "[DONE]" {
        return match state {
            SseStreamState::AwaitingData => Err(streaming_error(
                "provider stream completed without payload data",
            )),
            SseStreamState::Streaming => Err(streaming_error(
                "provider stream completed without a supported finish reason",
            )),
            SseStreamState::Terminal => Ok(EventAction::Done),
        };
    }

    let value: serde_json::Value = serde_json::from_str(data)
        .map_err(|_| streaming_error("provider returned malformed streaming data"))?;
    let Some(object) = value.as_object() else {
        return Err(streaming_error(
            "provider returned malformed streaming data",
        ));
    };
    if object
        .get("error")
        .is_some_and(|provider_error| !provider_error.is_null())
    {
        return Err(streaming_error("provider returned a streaming error"));
    }
    if !object.contains_key("choices") && object.get("usage").is_none_or(serde_json::Value::is_null)
    {
        return Err(streaming_error(
            "provider returned malformed streaming data",
        ));
    }

    let chunk = parse_stream_chunk(value)?;
    if *state == SseStreamState::Terminal {
        return process_terminal_trailer(&chunk, usage);
    }
    if chunk.choices.iter().any(|choice| {
        choice
            .delta
            .content
            .as_ref()
            .is_some_and(|content| !content.is_empty())
            || choice
                .delta
                .visible_reasoning()
                .is_ok_and(|reasoning| reasoning.is_some_and(|value| !value.is_empty()))
            || !choice.delta.tool_calls.is_empty()
            || choice.delta.function_call.is_some()
            || (dialect == OpenAiCompatibleDialect::OpenRouter
                && preserve_opaque_reasoning_state
                && (choice
                    .delta
                    .visible_reasoning()
                    .is_ok_and(|reasoning| reasoning.is_some())
                    || choice.delta.reasoning_details.is_some()))
    }) {
        state.observe_payload();
    }
    update_usage(chunk.usage.as_ref(), usage);
    emit_choice_deltas(
        &chunk.choices,
        tool_calls,
        reasoning_details,
        dialect,
        preserve_opaque_reasoning_state,
        sink,
        cancelled,
        cancellation_open,
    )
    .await?;
    observe_finish_reasons(
        &chunk.choices,
        state,
        tool_calls,
        reasoning_details,
        sink,
        cancelled,
        cancellation_open,
    )
    .await?;
    Ok(EventAction::Continue)
}

fn parse_stream_chunk(value: serde_json::Value) -> CoreResult<StreamChunk> {
    let chunk: StreamChunk = serde_json::from_value(value)
        .map_err(|_| streaming_error("provider returned malformed streaming data"))?;
    validate_choice_selection(&chunk.choices)?;
    Ok(chunk)
}

fn validate_choice_selection(choices: &[StreamChoice]) -> CoreResult<()> {
    if choices.is_empty() {
        return Ok(());
    }
    if choices.len() != 1 || choices[0].index != 0 {
        return Err(streaming_error(
            "provider returned an unsupported choice selection",
        ));
    }
    choices[0].delta.visible_reasoning()?;
    Ok(())
}

fn process_terminal_trailer(
    chunk: &StreamChunk,
    usage: &mut GenerationUsage,
) -> CoreResult<EventAction> {
    if !chunk.choices.is_empty() {
        return Err(streaming_error(
            "provider returned choice data after a terminal finish reason",
        ));
    }
    update_usage(chunk.usage.as_ref(), usage);
    Ok(EventAction::Continue)
}

fn update_usage(stream_usage: Option<&StreamUsage>, usage: &mut GenerationUsage) {
    if let Some(stream_usage) = stream_usage {
        usage.input_tokens = stream_usage.prompt_tokens.or(usage.input_tokens);
        usage.output_tokens = stream_usage.completion_tokens.or(usage.output_tokens);
        let cached_tokens = stream_usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens);
        let reasoning_tokens = stream_usage
            .completion_tokens_details
            .as_ref()
            .and_then(|details| details.reasoning_tokens);
        let cache_write_tokens = stream_usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cache_write_tokens);
        usage.cached_read_tokens = cached_tokens.or(usage.cached_read_tokens);
        usage.cached_write_tokens = cache_write_tokens.or(usage.cached_write_tokens);
        usage.reasoning_tokens = reasoning_tokens.or(usage.reasoning_tokens);
        usage.provider_raw_summary = merge_usage_summary(
            usage.provider_raw_summary.as_ref(),
            &[
                ("total_tokens", stream_usage.total_tokens),
                (
                    "prompt_tokens_details.audio_tokens",
                    stream_usage
                        .prompt_tokens_details
                        .as_ref()
                        .and_then(|details| details.audio_tokens),
                ),
                (
                    "completion_tokens_details.audio_tokens",
                    stream_usage
                        .completion_tokens_details
                        .as_ref()
                        .and_then(|details| details.audio_tokens),
                ),
                (
                    "completion_tokens_details.accepted_prediction_tokens",
                    stream_usage
                        .completion_tokens_details
                        .as_ref()
                        .and_then(|details| details.accepted_prediction_tokens),
                ),
                (
                    "completion_tokens_details.rejected_prediction_tokens",
                    stream_usage
                        .completion_tokens_details
                        .as_ref()
                        .and_then(|details| details.rejected_prediction_tokens),
                ),
            ],
        );
    }
}

async fn observe_finish_reasons(
    choices: &[StreamChoice],
    state: &mut SseStreamState,
    tool_calls: &mut OpenAiToolCallTracker,
    reasoning_details: &mut OpenRouterReasoningTracker,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<()> {
    let mut saw_supported_finish_reason = false;
    let mut saw_tool_finish_reason = false;
    for choice in choices {
        match choice.finish_reason.as_deref() {
            None => {}
            Some("stop" | "length" | "content_filter") => {
                saw_supported_finish_reason = true;
            }
            Some("tool_calls" | "function_call") => {
                saw_supported_finish_reason = true;
                saw_tool_finish_reason = true;
            }
            Some(_) => {
                return Err(streaming_error(
                    "provider returned an unsupported finish reason",
                ));
            }
        }
    }
    if saw_supported_finish_reason {
        if saw_tool_finish_reason && tool_calls.calls.is_empty() {
            return Err(streaming_error(
                "provider ended for tool calls without returning a tool call",
            ));
        }
        let opaque_events = reasoning_details.finish(!tool_calls.calls.is_empty())?;
        send_provider_events(sink, opaque_events, cancelled, cancellation_open).await?;
        let events = tool_calls.finish_all()?;
        send_provider_events(sink, events, cancelled, cancellation_open).await?;
        *state = SseStreamState::Terminal;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn emit_choice_deltas(
    choices: &[StreamChoice],
    tool_calls: &mut OpenAiToolCallTracker,
    reasoning_details: &mut OpenRouterReasoningTracker,
    dialect: OpenAiCompatibleDialect,
    preserve_opaque_reasoning_state: bool,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<()> {
    for choice in choices {
        let visible_reasoning = choice.delta.visible_reasoning()?;
        reasoning_details.observe(
            visible_reasoning,
            choice.delta.reasoning_details.as_ref(),
            dialect == OpenAiCompatibleDialect::OpenRouter && preserve_opaque_reasoning_state,
        )?;
        if let Some(reasoning) = visible_reasoning.filter(|value| !value.is_empty()) {
            send_provider_event(
                sink,
                ProviderEvent::ReasoningDelta(reasoning.to_owned()),
                cancelled,
                cancellation_open,
            )
            .await?;
        }
        if let Some(content) = choice
            .delta
            .content
            .as_ref()
            .filter(|value| !value.is_empty())
        {
            send_provider_event(
                sink,
                ProviderEvent::TextDelta(content.clone()),
                cancelled,
                cancellation_open,
            )
            .await?;
        }
        let tool_events = tool_calls.observe_choice(&choice.delta)?;
        send_provider_events(sink, tool_events, cancelled, cancellation_open).await?;
    }
    Ok(())
}

impl OpenAiToolCallTracker {
    fn observe_choice(&mut self, delta: &StreamDelta) -> CoreResult<Vec<ProviderEvent>> {
        let mut events = Vec::new();
        for (position, tool_delta) in delta.tool_calls.iter().enumerate() {
            let fallback_index = u32::try_from(position)
                .map_err(|_| streaming_error("provider returned too many tool calls"))?;
            let index = tool_delta.index.unwrap_or(fallback_index);
            if index == LEGACY_FUNCTION_CALL_INDEX {
                return Err(streaming_error(
                    "provider returned an invalid tool-call index",
                ));
            }
            events.extend(self.observe_delta(
                index,
                None,
                tool_delta.kind.as_deref(),
                tool_delta.id.as_deref(),
                tool_delta.function.as_ref(),
            )?);
        }
        if let Some(function) = delta.function_call.as_ref() {
            events.extend(self.observe_delta(
                LEGACY_FUNCTION_CALL_INDEX,
                Some(LEGACY_FUNCTION_CALL_ID),
                Some("function"),
                None,
                Some(function),
            )?);
        }
        Ok(events)
    }

    fn observe_delta(
        &mut self,
        index: u32,
        default_id: Option<&str>,
        kind: Option<&str>,
        id_fragment: Option<&str>,
        function: Option<&OpenAiFunctionCallDelta>,
    ) -> CoreResult<Vec<ProviderEvent>> {
        if kind.is_some_and(|kind| kind != "function") {
            return Err(streaming_error(
                "provider returned an unsupported tool-call type",
            ));
        }
        let partial = self.calls.entry(index).or_default();
        if partial.id.is_empty()
            && let Some(default_id) = default_id
        {
            partial.id.push_str(default_id);
        }
        let id_fragment = id_fragment.filter(|fragment| !fragment.is_empty());
        let name_fragment = function
            .and_then(|function| function.name.as_deref())
            .filter(|fragment| !fragment.is_empty());
        if partial.started.is_some() {
            if id_fragment.is_some_and(|fragment| fragment != partial.id)
                || name_fragment.is_some_and(|fragment| fragment != partial.name)
            {
                return Err(streaming_error(
                    "provider changed tool-call identity after arguments began",
                ));
            }
        } else if let Some(fragment) = id_fragment {
            append_bounded_fragment(
                &mut partial.id,
                fragment,
                MAX_TOOL_CALL_ID_BYTES,
                "provider tool-call id exceeded its safety limit",
            )?;
        }
        if partial.started.is_none()
            && let Some(fragment) = name_fragment
        {
            append_bounded_fragment(
                &mut partial.name,
                fragment,
                MAX_TOOL_NAME_BYTES,
                "provider tool name exceeded its safety limit",
            )?;
        }
        if let Some(arguments) = function
            .and_then(|function| function.arguments.as_deref())
            .filter(|arguments| !arguments.is_empty())
        {
            let delta = ToolCallArgumentsDelta::parse(arguments.to_owned())
                .map_err(|_| streaming_error("provider returned invalid tool-call arguments"))?;
            partial.pending_argument_bytes = partial
                .pending_argument_bytes
                .checked_add(arguments.len())
                .filter(|size| *size <= MAX_PENDING_TOOL_ARGUMENT_BYTES)
                .ok_or_else(|| {
                    streaming_error("provider tool-call arguments exceeded its safety limit")
                })?;
            partial.pending_arguments.push(delta);
        }
        self.start_if_ready(index, false)
    }

    fn start_if_ready(
        &mut self,
        index: u32,
        require_complete_identity: bool,
    ) -> CoreResult<Vec<ProviderEvent>> {
        let partial = self
            .calls
            .get(&index)
            .ok_or_else(|| streaming_error("provider returned malformed tool-call state"))?;
        if partial.started.is_some() {
            let id = partial
                .started
                .clone()
                .ok_or_else(|| streaming_error("provider returned malformed tool-call state"))?;
            let pending = self
                .calls
                .get_mut(&index)
                .map(|partial| {
                    partial.pending_argument_bytes = 0;
                    std::mem::take(&mut partial.pending_arguments)
                })
                .unwrap_or_default();
            return Ok(pending
                .into_iter()
                .map(|delta| ProviderEvent::ToolCallArgumentsDelta {
                    id: id.clone(),
                    delta,
                })
                .collect());
        }
        if partial.id.is_empty() || partial.name.is_empty() {
            if require_complete_identity {
                return Err(streaming_error(
                    "provider returned an incomplete tool-call identity",
                ));
            }
            return Ok(Vec::new());
        }
        // Wait until the first argument fragment or the terminal marker. This
        // lets compatible providers split a function name across chunks
        // without exposing a prematurely truncated public name.
        if partial.pending_arguments.is_empty() && !require_complete_identity {
            return Ok(Vec::new());
        }

        let id = ToolCallId::parse(partial.id.clone())
            .map_err(|_| streaming_error("provider returned an invalid tool-call id"))?;
        let name = ToolName::parse(partial.name.clone())
            .map_err(|_| streaming_error("provider returned an invalid tool name"))?;
        if !self.started_ids.insert(id.clone()) {
            return Err(streaming_error("provider reused a tool-call identifier"));
        }
        let pending = self
            .calls
            .get_mut(&index)
            .map(|partial| {
                partial.started = Some(id.clone());
                partial.pending_argument_bytes = 0;
                std::mem::take(&mut partial.pending_arguments)
            })
            .unwrap_or_default();
        let mut events = Vec::with_capacity(pending.len().saturating_add(1));
        events.push(ProviderEvent::ToolCallStarted {
            id: id.clone(),
            name,
        });
        events.extend(
            pending
                .into_iter()
                .map(|delta| ProviderEvent::ToolCallArgumentsDelta {
                    id: id.clone(),
                    delta,
                }),
        );
        Ok(events)
    }

    fn finish_all(&mut self) -> CoreResult<Vec<ProviderEvent>> {
        let indexes = self.calls.keys().copied().collect::<Vec<_>>();
        let mut events = Vec::new();
        for index in indexes {
            events.extend(self.start_if_ready(index, true)?);
            let id = self
                .calls
                .get(&index)
                .and_then(|partial| partial.started.clone())
                .ok_or_else(|| streaming_error("provider returned malformed tool-call state"))?;
            events.push(ProviderEvent::ToolCallCompleted { id });
        }
        Ok(events)
    }
}

fn append_bounded_fragment(
    target: &mut String,
    fragment: &str,
    max_bytes: usize,
    error_message: &'static str,
) -> CoreResult<()> {
    target
        .len()
        .checked_add(fragment.len())
        .filter(|size| *size <= max_bytes)
        .ok_or_else(|| streaming_error(error_message))?;
    target.push_str(fragment);
    Ok(())
}

async fn send_provider_events(
    sink: &ProviderEventSender,
    events: Vec<ProviderEvent>,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<()> {
    for event in events {
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
    if size > MAX_SSE_BUFFER_BYTES {
        return Err(streaming_error("provider streaming event exceeded 1 MiB"));
    }
    Ok(())
}

fn ensure_pending_size(bytes: &[u8], end_of_stream: bool) -> CoreResult<()> {
    if bytes.len() <= MAX_SSE_BUFFER_BYTES {
        return Ok(());
    }
    let possible_separator = &bytes[MAX_SSE_BUFFER_BYTES..];
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
            "provider returned an unexpected content type",
        ))
    }
}

fn streaming_error(message: &'static str) -> CoreError {
    CoreError::new(CoreErrorCode::ProviderUnavailable, message, true)
}

fn validate_endpoint(endpoint: &Url) -> CoreResult<()> {
    if endpoint.username() != "" || endpoint.password().is_some() {
        return Err(CoreError::invalid(
            "provider URL must not contain embedded credentials",
        ));
    }
    match endpoint.scheme() {
        "https" => Ok(()),
        "http" if endpoint.host_str().is_some_and(is_loopback_host) => Ok(()),
        "http" => Err(CoreError::invalid(
            "unencrypted HTTP is allowed only for loopback endpoints",
        )),
        _ => Err(CoreError::invalid(
            "provider endpoint must use HTTPS or loopback HTTP",
        )),
    }
}

fn is_loopback_host(host: &str) -> bool {
    if let Ok(address) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        return address.is_loopback();
    }
    let host = host.trim_end_matches('.');
    host.eq_ignore_ascii_case("localhost")
        || host
            .to_ascii_lowercase()
            .strip_suffix(".localhost")
            .is_some_and(|prefix| !prefix.is_empty())
}

pub(crate) const fn prompt_contract() -> ProviderPromptAdapterContract {
    ProviderPromptAdapterContract::for_family(ApiFamily::OpenAiChatCompletions)
}

#[cfg(test)]
pub(crate) fn resolved_prompt_payload_for_contract_test(
    request: GenerationRequest,
    cache_dialect: PromptCacheWireDialect,
) -> CoreResult<serde_json::Value> {
    validate_request(&request, OpenAiCompatibleDialect::OpenRouter)?;
    let payload =
        request_payload_with_cache(request, OpenAiCompatibleDialect::OpenRouter, cache_dialect)?;
    serde_json::to_value(payload)
        .map_err(|_| CoreError::internal("failed to serialize OpenAI-compatible test payload"))
}

fn role_name(role: MessageRole) -> &'static str {
    prompt_contract().map_message_role(role).wire_role.as_str()
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
            "provider network request failed",
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
        thread,
    };

    use lorepia_domain::{
        ApiFamily, ConversationId, GenerationId, GenerationPresetId, GenerationProviderProvenance,
        GenerationRequest, Message, MessageStatus, ModelRouteId, OpaqueReasoningContext,
    };
    use tokio::sync::{mpsc, watch};

    use super::*;

    #[test]
    fn rejects_credentials_and_remote_plain_http() {
        assert!(
            OpenAiCompatibleProvider::new(
                "https://user:secret@example.com/v1",
                Duration::from_secs(1)
            )
            .is_err()
        );
        assert!(
            OpenAiCompatibleProvider::new("http://example.com/v1", Duration::from_secs(1)).is_err()
        );
        assert!(
            OpenAiCompatibleProvider::new("http://127.0.0.1:11434/v1", Duration::from_secs(1))
                .is_ok()
        );
        assert!(
            OpenAiCompatibleProvider::new("http://127.0.0.2:11434/v1", Duration::from_secs(1))
                .is_ok()
        );
        assert!(
            OpenAiCompatibleProvider::new(
                "http://provider.localhost:11434/v1",
                Duration::from_secs(1)
            )
            .is_ok()
        );
    }

    fn request() -> GenerationRequest {
        GenerationRequest {
            generation_id: GenerationId::new(),
            conversation_id: ConversationId::new(),
            model: "fixture".to_owned(),
            messages: Vec::new(),
            resolved_prompt_plan: None,
            provider_execution_plan_hash: None,
            temperature: Some(1.0),
            max_output_tokens: None,
            provider_provenance: None,
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        }
    }

    fn read_http_request(stream: &mut TcpStream) -> io::Result<()> {
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
        Ok(())
    }

    fn status_server(status: &str, extra_headers: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind status server");
        let address = listener.local_addr().expect("status server address");
        let status = status.to_owned();
        let extra_headers = extra_headers.to_owned();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            read_http_request(&mut stream).expect("read status request");
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n{extra_headers}\r\n"
            )
            .expect("write status");
        });
        format!("http://{address}/v1")
    }

    fn stream_server(body: &[u8], fragment_bytes: usize) -> String {
        stream_server_with_content_type(body, fragment_bytes, "text/event-stream")
    }

    fn stream_server_with_content_type(
        body: &[u8],
        fragment_bytes: usize,
        content_type: &str,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stream server");
        let address = listener.local_addr().expect("stream server address");
        let content_type = content_type.to_owned();
        let chunks = body
            .chunks(fragment_bytes)
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            read_http_request(&mut stream).expect("read stream request");
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
        format!("http://{address}/v1")
    }

    async fn generate_from_stream(
        body: &[u8],
        fragment_bytes: usize,
    ) -> (CoreResult<GenerationUsage>, Vec<ProviderEvent>) {
        generate_from_stream_with_timeout(body, fragment_bytes, Duration::from_secs(2)).await
    }

    async fn generate_from_stream_with_timeout(
        body: &[u8],
        fragment_bytes: usize,
        timeout: Duration,
    ) -> (CoreResult<GenerationUsage>, Vec<ProviderEvent>) {
        let provider = OpenAiCompatibleProvider::new(&stream_server(body, fragment_bytes), timeout)
            .expect("provider");
        let (sink, mut events) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let result = provider.generate(request(), None, sink, cancelled).await;
        let mut received = Vec::new();
        while let Ok(event) = events.try_recv() {
            received.push(event);
        }
        (result, received)
    }

    async fn status_error_from(status: &str, extra_headers: &str) -> CoreError {
        let provider = OpenAiCompatibleProvider::new(
            &status_server(status, extra_headers),
            Duration::from_secs(2),
        )
        .expect("provider");
        let (sink, _events) = mpsc::channel(4);
        let (_cancel, cancelled) = watch::channel(false);
        provider
            .generate(request(), None, sink, cancelled)
            .await
            .expect_err("status must fail")
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
        let body = b"{\"choices\":[]}";
        let provider = OpenAiCompatibleProvider::new(
            &stream_server_with_content_type(body, body.len(), "application/json"),
            Duration::from_secs(2),
        )
        .expect("provider");
        let (sink, _events) = mpsc::channel(4);
        let (_cancel, cancelled) = watch::channel(false);

        let error = provider
            .generate(request(), None, sink, cancelled)
            .await
            .expect_err("content type must be event-stream");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(
            error.message,
            "provider returned an unexpected content type"
        );
    }

    #[tokio::test]
    async fn streams_fragmented_lf_and_crlf_events() {
        let body = concat!(
            ": keepalive\n\r\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"생각\"}}]}\r\n\r\n",
            ": still-alive\r\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"안녕\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5,\"prompt_tokens_details\":{\"cached_tokens\":2,\"cache_write_tokens\":4,\"audio_tokens\":1},\"completion_tokens_details\":{\"reasoning_tokens\":1,\"audio_tokens\":2,\"accepted_prediction_tokens\":3,\"rejected_prediction_tokens\":4}}}\r\n\r\n",
            "data: [DONE]\n\n",
        );

        let (result, events) = generate_from_stream(body.as_bytes(), 3).await;

        let usage = result.expect("valid stream");
        assert_eq!(usage.input_tokens, Some(3));
        assert_eq!(usage.cached_read_tokens, Some(2));
        assert_eq!(usage.cached_write_tokens, Some(4));
        assert_eq!(usage.output_tokens, Some(2));
        assert_eq!(usage.reasoning_tokens, Some(1));
        assert_eq!(usage.tool_tokens, None);
        assert_eq!(
            usage
                .provider_raw_summary
                .as_ref()
                .map(lorepia_domain::BoundedJson::as_str),
            Some(
                r#"{"completion_tokens_details.accepted_prediction_tokens":3,"completion_tokens_details.audio_tokens":2,"completion_tokens_details.rejected_prediction_tokens":4,"prompt_tokens_details.audio_tokens":1,"total_tokens":5}"#
            )
        );
        assert_eq!(
            events,
            vec![
                ProviderEvent::ReasoningDelta("생각".to_owned()),
                ProviderEvent::TextDelta("안녕".to_owned()),
            ]
        );
    }

    #[tokio::test]
    async fn streams_bare_cr_and_mixed_line_endings() {
        let body = concat!(
            ": bare-cr keepalive\r\r",
            "event: message\rdata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"CR\"}}]}\r\r",
            ": mixed keepalive\r\r\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":1}}\n\r",
            "data: [DONE]\r\n\r",
        );

        let (result, events) = generate_from_stream(body.as_bytes(), 1).await;

        assert_eq!(
            result.expect("valid bare-CR stream"),
            GenerationUsage {
                input_tokens: Some(4),
                output_tokens: Some(1),
                ..GenerationUsage::default()
            }
        );
        assert_eq!(events, vec![ProviderEvent::TextDelta("CR".to_owned())]);
    }

    #[test]
    fn recognizes_complete_bare_cr_boundary_at_a_chunk_edge() {
        assert_eq!(
            find_event_boundary(b"data: x\r\r", false),
            Some((b"data: x".len(), 2))
        );
        assert_eq!(find_event_boundary(b"data: x\r", false), None);
        assert_eq!(
            find_event_boundary(b"data: x\r\n\r\n", false),
            Some((b"data: x".len(), 4))
        );
    }

    #[tokio::test]
    async fn first_done_ignores_late_data_already_buffered_with_it() {
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"before\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"late\"}}]}\n\n",
            "data: {\"error\":{\"message\":\"also late\"}}\n\n",
        );

        let (result, events) = generate_from_stream(body.as_bytes(), body.len()).await;

        result.expect("the first terminal marker safely ends the stream");
        assert_eq!(events, vec![ProviderEvent::TextDelta("before".to_owned())]);
    }

    #[tokio::test]
    async fn rejects_empty_and_keepalive_only_success_responses() {
        for body in [
            b"".as_slice(),
            b": keepalive\r\n\r\n".as_slice(),
            b": bare-cr keepalive\r\r".as_slice(),
        ] {
            let (result, events) = generate_from_stream(body, 2).await;
            let error = result.expect_err("empty response must fail");

            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
            assert_eq!(
                error.message, "provider returned an empty streaming response",
                "body: {body:?}"
            );
            assert!(events.is_empty());
        }
    }

    #[tokio::test]
    async fn rejects_stream_that_ends_without_done() {
        let body =
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"},\"finish_reason\":\"stop\"}]}\n\n";

        let (result, events) = generate_from_stream(body, 5).await;
        let error = result.expect_err("unterminated stream must fail");

        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(error.message, "provider stream ended before [DONE]");
        assert_eq!(events, vec![ProviderEvent::TextDelta("partial".to_owned())]);
    }

    #[tokio::test]
    async fn rejects_done_event_without_a_terminating_blank_line() {
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]",
        );

        let (result, events) = generate_from_stream(body.as_bytes(), 7).await;
        let error = result.expect_err("incomplete terminal event must fail");

        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(
            error.message,
            "provider stream ended with an incomplete event"
        );
        assert_eq!(events, vec![ProviderEvent::TextDelta("partial".to_owned())]);
    }

    #[tokio::test]
    async fn rejects_done_without_payload_data() {
        let (result, events) = generate_from_stream(b"data: [DONE]\n\n", 1).await;
        let error = result.expect_err("payload-free stream must fail");

        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(
            error.message,
            "provider stream completed without payload data"
        );
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn rejects_done_after_payload_without_a_supported_finish_reason() {
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\n",
            "data: [DONE]\n\n",
        );

        let (result, events) = generate_from_stream(body.as_bytes(), 3).await;
        let error = result.expect_err("a terminal marker alone must not complete generation");

        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(
            error.message,
            "provider stream completed without a supported finish reason"
        );
        assert_eq!(events, vec![ProviderEvent::TextDelta("partial".to_owned())]);
    }

    #[tokio::test]
    async fn rejects_empty_choices_as_payload_free() {
        let bodies = [
            b"data: {\"choices\":[]}\n\ndata: [DONE]\n\n".as_slice(),
            b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":0}}\n\ndata: [DONE]\n\n"
                .as_slice(),
            b"data: {\"choices\":[{\"index\":0,\"delta\":{}}]}\n\ndata: [DONE]\n\n".as_slice(),
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\ndata: [DONE]\n\n"
                .as_slice(),
        ];

        for body in bodies {
            let (result, events) = generate_from_stream(body, 4).await;
            let error = result.expect_err("empty choices must not establish payload data");

            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
            assert_eq!(
                error.message,
                "provider stream completed without payload data"
            );
            assert!(events.is_empty());
        }
    }

    #[tokio::test]
    async fn accepts_standard_terminal_finish_reasons() {
        for finish_reason in ["length", "content_filter"] {
            let body = format!(
                "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"terminal\"}},\"finish_reason\":\"{finish_reason}\"}}]}}\n\ndata: [DONE]\n\n"
            );

            let (result, events) = generate_from_stream(body.as_bytes(), 5).await;

            result.expect("standard terminal reason must complete generation");
            assert_eq!(
                events,
                vec![ProviderEvent::TextDelta("terminal".to_owned())],
                "finish reason: {finish_reason}"
            );
        }
    }

    #[tokio::test]
    async fn accepts_only_empty_choice_trailers_after_a_terminal_reason() {
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"complete\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3}}\n\n",
            "data: [DONE]\n\n",
        );

        let (result, events) = generate_from_stream(body.as_bytes(), body.len()).await;

        assert_eq!(
            result.expect("usage-only trailer must be accepted"),
            GenerationUsage {
                input_tokens: Some(7),
                output_tokens: Some(3),
                ..GenerationUsage::default()
            }
        );
        assert_eq!(
            events,
            vec![ProviderEvent::TextDelta("complete".to_owned())]
        );
    }

    #[tokio::test]
    async fn rejects_choice_data_after_a_terminal_reason_without_emitting_it() {
        for late_choice in [
            "{\"choices\":[{\"index\":0,\"delta\":{\"content\":\"late text\"}}]}",
            "{\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"late reasoning\"}}]}",
            "{\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}",
        ] {
            let body = format!(
                "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"complete\"}},\"finish_reason\":\"stop\"}}]}}\n\ndata: {late_choice}\n\ndata: [DONE]\n\n"
            );

            let (result, events) = generate_from_stream(body.as_bytes(), body.len()).await;
            let error = result.expect_err("choice data after terminal reason must fail");

            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
            assert_eq!(
                error.message,
                "provider returned choice data after a terminal finish reason"
            );
            assert_eq!(
                events,
                vec![ProviderEvent::TextDelta("complete".to_owned())],
                "late choice: {late_choice}"
            );
        }
    }

    #[tokio::test]
    async fn represents_streamed_and_legacy_tool_calls_as_inert_protocol_events() {
        let cases = [
            (
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"arguments\":\"\"}}]}}]}\n\n",
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"q\\\":\"}}]}}]}\n\n",
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"seoul\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
                    "data: [DONE]\n\n",
                ),
                "call-1",
            ),
            (
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"function_call\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":\\\"seoul\\\"}\"}},\"finish_reason\":\"function_call\"}]}\n\n",
                    "data: [DONE]\n\n",
                ),
                LEGACY_FUNCTION_CALL_ID,
            ),
        ];

        for (body, expected_id) in cases {
            let (result, events) = generate_from_stream(body.as_bytes(), 5).await;
            result.expect("tool-call finish reason must be represented normally");

            assert!(matches!(
                &events[0],
                ProviderEvent::ToolCallStarted { id, name }
                    if id.as_str() == expected_id && name.as_str() == "lookup"
            ));
            assert!(events.iter().any(|event| matches!(
                event,
                ProviderEvent::ToolCallArgumentsDelta { id, .. }
                    if id.as_str() == expected_id
            )));
            assert!(matches!(
                events.last(),
                Some(ProviderEvent::ToolCallCompleted { id }) if id.as_str() == expected_id
            ));
        }
    }

    #[tokio::test]
    async fn accepts_stop_finish_reason_and_null_error_field() {
        let body = concat!(
            "data: {\"error\":null,\"choices\":[{\"index\":0,\"delta\":{\"content\":\"complete\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );

        let (result, events) = generate_from_stream(body.as_bytes(), 3).await;

        result.expect("stop is a supported terminal reason");
        assert_eq!(
            events,
            vec![ProviderEvent::TextDelta("complete".to_owned())]
        );

        let empty_stop =
            b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let (result, events) = generate_from_stream(empty_stop, 2).await;

        result.expect("an explicit stop may complete with empty content");
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn rejects_malformed_data_and_streaming_error_envelopes() {
        let cases = [
            (
                b"data: {not-json}\n\n".as_slice(),
                "provider returned malformed streaming data",
            ),
            (
                b"data: {\"error\":{\"message\":\"synthetic failure\"}}\n\n".as_slice(),
                "provider returned a streaming error",
            ),
            (
                b"data: {\"unexpected\":true}\n\n".as_slice(),
                "provider returned malformed streaming data",
            ),
        ];

        for (body, expected_message) in cases {
            let (result, events) = generate_from_stream(body, 4).await;
            let error = result.expect_err("invalid stream must fail");

            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
            assert_eq!(error.message, expected_message);
            assert!(events.is_empty());
        }
    }

    #[test]
    fn validates_bounded_inputs_and_rejects_unsupported_opaque_preference() {
        let mut empty_preference = request();
        empty_preference.preserve_opaque_reasoning_state = true;
        let error = validate_request(&empty_preference, OpenAiCompatibleDialect::Standard)
            .expect_err("standard routes must reject unsupported opaque-state preservation");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            error.message,
            "this OpenAI-compatible route cannot preserve opaque reasoning state"
        );

        let mut oversized_model = request();
        oversized_model.model = "m".repeat(MAX_MODEL_ID_BYTES + 1);
        assert_eq!(
            validate_request(&oversized_model, OpenAiCompatibleDialect::Standard)
                .expect_err("oversized model")
                .code,
            CoreErrorCode::InvalidInput
        );

        let mut too_many_messages = request();
        too_many_messages.messages = (0..=MAX_PROMPT_MESSAGES)
            .map(|_| Message::user(too_many_messages.conversation_id.clone(), "x"))
            .collect();
        assert_eq!(
            validate_request(&too_many_messages, OpenAiCompatibleDialect::Standard)
                .expect_err("message count")
                .code,
            CoreErrorCode::InvalidInput
        );

        let mut oversized_prompt = request();
        oversized_prompt.messages.push(Message::user(
            oversized_prompt.conversation_id.clone(),
            "x".repeat(MAX_PROMPT_BYTES + 1),
        ));
        assert_eq!(
            validate_request(&oversized_prompt, OpenAiCompatibleDialect::Standard)
                .expect_err("prompt size")
                .code,
            CoreErrorCode::InvalidInput
        );

        let mut cross_conversation = request();
        cross_conversation
            .messages
            .push(Message::user(ConversationId::new(), "wrong conversation"));
        assert_eq!(
            validate_request(&cross_conversation, OpenAiCompatibleDialect::Standard)
                .expect_err("cross-conversation prompt")
                .code,
            CoreErrorCode::InvalidInput
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn openrouter_replays_typed_reasoning_details_in_original_assistant_order() {
        let standard_payload = serde_json::to_value(
            request_payload(request(), OpenAiCompatibleDialect::Standard)
                .expect("standard request payload"),
        )
        .expect("standard payload JSON");
        assert!(standard_payload.get("stream_options").is_none());

        let summary_canary = "private-summary-canary";
        let encrypted_canary = "private-encrypted-canary";
        let mut request = request();
        let user = Message::user(request.conversation_id.clone(), "question");
        let mut assistant = Message::pending_assistant(
            request.conversation_id.clone(),
            user.id.clone(),
            GenerationId::new(),
        );
        assistant.content = "prior answer".to_owned();
        assistant.status = MessageStatus::Complete;
        let route_id = ModelRouteId::from("openrouter-route");
        request.messages = vec![user, assistant.clone()];
        request.provider_provenance = Some(GenerationProviderProvenance {
            api_family: ApiFamily::OpenAiChatCompletions,
            model_route_id: route_id.clone(),
            generation_preset_id: GenerationPresetId::from("current-preset"),
        });
        request.preserve_opaque_reasoning_state = true;
        let detail_values = [
            serde_json::json!({
                "type": "reasoning.summary",
                "summary": summary_canary,
                "id": "summary-1",
                "format": "anthropic-claude-v1",
                "index": 0
            }),
            serde_json::json!({
                "type": "reasoning.encrypted",
                "data": encrypted_canary,
                "id": "encrypted-1",
                "format": "anthropic-claude-v1",
                "index": 1
            }),
        ];
        let details = detail_values
            .iter()
            .map(|value| {
                OpenRouterReasoningDetail::from_value(value).expect("valid OpenRouter detail")
            })
            .collect();
        let plaintext_canary = "private-plaintext-canary";
        request.opaque_reasoning_context = vec![OpaqueReasoningContext {
            source_message_id: assistant.id.clone(),
            api_family: ApiFamily::OpenAiChatCompletions,
            model: request.model.clone(),
            model_route_id: route_id.clone(),
            generation_preset_id: GenerationPresetId::from("prior-preset"),
            state: OpaqueReasoningState::OpenRouterReasoning {
                topology: OpenRouterReasoningTopology::new(
                    Some(plaintext_canary.to_owned()),
                    Some(details),
                )
                .expect("valid OpenRouter topology"),
            },
        }];

        validate_request(&request, OpenAiCompatibleDialect::OpenRouter)
            .expect("exact OpenRouter context");
        let generic = serde_json::to_string(&request).expect("generic request JSON");
        let debug = format!("{request:?}");
        for canary in [summary_canary, encrypted_canary, plaintext_canary] {
            assert!(!generic.contains(canary));
            assert!(!debug.contains(canary));
        }

        let payload = serde_json::to_value(
            request_payload(request.clone(), OpenAiCompatibleDialect::OpenRouter)
                .expect("OpenRouter request payload"),
        )
        .expect("payload JSON");
        assert_eq!(
            payload["messages"][1]["reasoning_details"],
            serde_json::Value::Array(detail_values.to_vec())
        );
        assert_eq!(payload["messages"][1]["reasoning"], plaintext_canary);
        assert_eq!(payload["stream_options"]["include_usage"], true);

        let mut empty_marker = request.clone();
        empty_marker.opaque_reasoning_context[0].state =
            OpaqueReasoningState::OpenRouterReasoning {
                topology: OpenRouterReasoningTopology::new(None, Some(Vec::new()))
                    .expect("an observed empty detail array is meaningful"),
            };
        let empty_payload = serde_json::to_value(
            request_payload(empty_marker, OpenAiCompatibleDialect::OpenRouter)
                .expect("empty marker request payload"),
        )
        .expect("empty marker JSON");
        assert_eq!(
            empty_payload["messages"][1]["reasoning_details"],
            serde_json::json!([])
        );
        assert!(empty_payload["messages"][1].get("reasoning").is_none());

        for signature_only in [
            serde_json::json!({
                "type": "reasoning.text",
                "signature": "missing-text-signature"
            }),
            serde_json::json!({
                "type": "reasoning.text",
                "text": null,
                "signature": "null-text-signature",
                "id": null,
                "format": null,
                "index": null
            }),
        ] {
            let mut exact_replay = request.clone();
            exact_replay.opaque_reasoning_context[0].state =
                OpaqueReasoningState::OpenRouterReasoning {
                    topology: OpenRouterReasoningTopology::new(
                        None,
                        Some(vec![
                            OpenRouterReasoningDetail::from_value(&signature_only)
                                .expect("signature-only text detail"),
                        ]),
                    )
                    .expect("signature-only topology"),
                };
            let payload = serde_json::to_value(
                request_payload(exact_replay, OpenAiCompatibleDialect::OpenRouter)
                    .expect("signature-only replay"),
            )
            .expect("signature-only payload");
            assert_eq!(
                payload["messages"][1]["reasoning_details"][0],
                signature_only
            );
        }

        let wrong_indexes = detail_values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let value = if index == 1 {
                    serde_json::json!({
                    "type": "reasoning.encrypted",
                    "data": encrypted_canary,
                    "id": "encrypted-1",
                    "format": "anthropic-claude-v1",
                    "index": 0
                    })
                } else {
                    value.clone()
                };
                OpenRouterReasoningDetail::from_value(&value).expect("individually valid detail")
            })
            .collect();
        assert!(
            OpenRouterReasoningTopology::new(None, Some(wrong_indexes)).is_err(),
            "stored detail indexes must be sequential"
        );

        let mut wrong_route = request;
        wrong_route.opaque_reasoning_context[0].model_route_id = ModelRouteId::from("other-route");
        assert_eq!(
            validate_request(&wrong_route, OpenAiCompatibleDialect::OpenRouter)
                .expect_err("cross-route state")
                .code,
            CoreErrorCode::InvalidInput
        );
        assert_eq!(
            validate_request(&wrong_route, OpenAiCompatibleDialect::Standard)
                .expect_err("OpenRouter state on a generic route")
                .code,
            CoreErrorCode::InvalidInput
        );

        wrong_route.opaque_reasoning_context[0].model_route_id = route_id;
        wrong_route.opaque_reasoning_context[0].source_message_id =
            wrong_route.messages[0].id.clone();
        assert_eq!(
            request_payload(wrong_route, OpenAiCompatibleDialect::OpenRouter)
                .err()
                .expect("state must stay on its original assistant message")
                .code,
            CoreErrorCode::InvalidInput
        );
    }

    #[tokio::test]
    async fn rejects_missing_nonzero_and_multiple_choices_before_emitting_events() {
        let invalid_chunks = [
            r#"data: {"choices":[{"delta":{"content":"private-choice-canary"}}]}"#,
            r#"data: {"choices":[{"index":1,"delta":{"content":"private-choice-canary","reasoning_details":[{"type":"reasoning.encrypted","data":"private-detail-canary"}]}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{"content":"private-choice-canary"}},{"index":1,"delta":{"reasoning_details":[{"type":"reasoning.encrypted","data":"private-detail-canary"}]}}]}"#,
        ];

        for invalid_chunk in invalid_chunks {
            let (sink, mut events) = mpsc::channel(8);
            let (_cancel_sender, mut cancelled) = watch::channel(false);
            let mut usage = GenerationUsage::default();
            let mut state = SseStreamState::default();
            let mut tool_calls = OpenAiToolCallTracker::default();
            let mut reasoning_details = OpenRouterReasoningTracker::default();
            let mut cancellation_open = true;

            let error = process_event(
                invalid_chunk.as_bytes(),
                &sink,
                &mut usage,
                &mut state,
                &mut tool_calls,
                &mut reasoning_details,
                OpenAiCompatibleDialect::OpenRouter,
                true,
                &mut cancelled,
                &mut cancellation_open,
            )
            .await
            .expect_err("choice selection must fail closed");

            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
            assert!(!error.message.contains("private-choice-canary"));
            assert!(!error.message.contains("private-detail-canary"));
            assert_eq!(state, SseStreamState::AwaitingData);
            assert_eq!(usage, GenerationUsage::default());
            assert!(tool_calls.calls.is_empty());
            assert!(reasoning_details.details.is_empty());
            assert!(reasoning_details.reasoning.is_none());
            assert!(matches!(
                events.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ));
        }
    }

    #[tokio::test]
    async fn accepts_canonical_openrouter_reasoning_and_rejects_conflicting_alias() {
        for preserve in [false, true] {
            let (sink, mut events) = mpsc::channel(4);
            let (_cancel_sender, mut cancelled) = watch::channel(false);
            let mut usage = GenerationUsage::default();
            let mut state = SseStreamState::default();
            let mut tool_calls = OpenAiToolCallTracker::default();
            let mut reasoning_details = OpenRouterReasoningTracker::default();
            let mut cancellation_open = true;

            process_event(
                br#"data: {"choices":[{"index":0,"delta":{"reasoning":"canonical reasoning"},"finish_reason":"stop"}]}"#,
                &sink,
                &mut usage,
                &mut state,
                &mut tool_calls,
                &mut reasoning_details,
                OpenAiCompatibleDialect::OpenRouter,
                preserve,
                &mut cancelled,
                &mut cancellation_open,
            )
            .await
            .expect("canonical reasoning");
            assert_eq!(
                events.try_recv().expect("reasoning event"),
                ProviderEvent::ReasoningDelta("canonical reasoning".to_owned())
            );
            if preserve {
                let ProviderEvent::OpaqueReasoningState(
                    OpaqueReasoningState::OpenRouterReasoning { topology },
                ) = events.try_recv().expect("plaintext topology")
                else {
                    panic!("expected OpenRouter plaintext topology");
                };
                assert_eq!(topology.reasoning(), Some("canonical reasoning"));
                assert!(topology.reasoning_details().is_none());
            }
            assert!(events.try_recv().is_err());
            assert_eq!(state, SseStreamState::Terminal);
        }

        let (sink, mut events) = mpsc::channel(4);
        let (_cancel_sender, mut cancelled) = watch::channel(false);
        let mut usage = GenerationUsage::default();
        let mut state = SseStreamState::default();
        let mut tool_calls = OpenAiToolCallTracker::default();
        let mut reasoning_details = OpenRouterReasoningTracker::default();
        let mut cancellation_open = true;
        let error = process_event(
            br#"data: {"choices":[{"index":0,"delta":{"reasoning":"canonical","reasoning_content":"conflicting alias"},"finish_reason":"stop"}]}"#,
            &sink,
            &mut usage,
            &mut state,
            &mut tool_calls,
            &mut reasoning_details,
            OpenAiCompatibleDialect::OpenRouter,
            true,
            &mut cancelled,
            &mut cancellation_open,
        )
        .await
        .expect_err("conflicting reasoning fields");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(state, SseStreamState::AwaitingData);
        assert!(matches!(
            events.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn openrouter_never_releases_reasoning_details_with_tool_topology() {
        let (sink, mut events) = mpsc::channel(8);
        let (_cancel_sender, mut cancelled) = watch::channel(false);
        let mut usage = GenerationUsage::default();
        let mut state = SseStreamState::default();
        let mut tool_calls = OpenAiToolCallTracker::default();
        let mut reasoning_details = OpenRouterReasoningTracker::default();
        let mut cancellation_open = true;
        let error = process_event(
            br#"data: {"choices":[{"index":0,"delta":{"reasoning_details":[{"type":"reasoning.encrypted","data":"private-detail-canary","id":"detail-1","format":"anthropic-claude-v1","index":0}],"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"lookup","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
            &sink,
            &mut usage,
            &mut state,
            &mut tool_calls,
            &mut reasoning_details,
            OpenAiCompatibleDialect::OpenRouter,
            true,
            &mut cancelled,
            &mut cancellation_open,
        )
        .await
        .expect_err("reasoning continuity with tools must fail closed");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert!(!error.message.contains("private-detail-canary"));
        assert!(reasoning_details.details.is_empty());
        assert!(reasoning_details.reasoning.is_none());
        let mut received = Vec::new();
        while let Ok(event) = events.try_recv() {
            received.push(event);
        }
        assert!(
            received
                .iter()
                .all(|event| !matches!(event, ProviderEvent::OpaqueReasoningState(_)))
        );
    }

    #[tokio::test]
    async fn openrouter_rejects_cross_chunk_duplicate_encrypted_indexes_before_release() {
        let (sink, mut events) = mpsc::channel(8);
        let (_cancel_sender, mut cancelled) = watch::channel(false);
        let mut usage = GenerationUsage::default();
        let mut state = SseStreamState::default();
        let mut tool_calls = OpenAiToolCallTracker::default();
        let mut reasoning_details = OpenRouterReasoningTracker::default();
        let mut cancellation_open = true;

        process_event(
            br#"data: {"choices":[{"index":0,"delta":{"reasoning_details":[{"type":"reasoning.encrypted","data":"private-first-canary","id":"detail-1","format":"anthropic-claude-v1","index":0}]}}]}"#,
            &sink,
            &mut usage,
            &mut state,
            &mut tool_calls,
            &mut reasoning_details,
            OpenAiCompatibleDialect::OpenRouter,
            true,
            &mut cancelled,
            &mut cancellation_open,
        )
        .await
        .expect("first detail");
        assert!(matches!(
            events.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        let error = process_event(
            br#"data: {"choices":[{"index":0,"delta":{"reasoning_details":[{"type":"reasoning.encrypted","data":"private-duplicate-canary","id":"detail-2","format":"anthropic-claude-v1","index":0}]},"finish_reason":"stop"}]}"#,
            &sink,
            &mut usage,
            &mut state,
            &mut tool_calls,
            &mut reasoning_details,
            OpenAiCompatibleDialect::OpenRouter,
            true,
            &mut cancelled,
            &mut cancellation_open,
        )
        .await
        .expect_err("duplicate detail index");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert!(!error.message.contains("private-first-canary"));
        assert!(!error.message.contains("private-duplicate-canary"));
        assert!(reasoning_details.details.is_empty());
        assert!(reasoning_details.reasoning.is_none());
        assert!(matches!(
            events.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn openrouter_stream_preserves_reasoning_detail_order_and_cache_usage() {
        let detail_values = vec![
            serde_json::json!({
                "type": "reasoning.text",
                "text": "private-reasoning-canary",
                "signature": null,
                "id": "text-1",
                "format": "anthropic-claude-v1",
                "index": 0
            }),
            serde_json::json!({
                "type": "reasoning.encrypted",
                "data": "private-encrypted-canary",
                "id": "encrypted-1",
                "format": "anthropic-claude-v1",
                "index": 1
            }),
        ];
        let event = format!(
            "data: {}",
            serde_json::json!({
                "choices": [{
                    "index": 0,
                    "delta": {"reasoning_details": detail_values.clone()},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 17,
                    "completion_tokens": 3,
                    "prompt_tokens_details": {
                        "cached_tokens": 11,
                        "cache_write_tokens": 6
                    }
                }
            })
        );
        let (sink, mut events) = mpsc::channel(4);
        let (_cancel_sender, mut cancelled) = watch::channel(false);
        let mut usage = GenerationUsage::default();
        let mut state = SseStreamState::default();
        let mut tool_calls = OpenAiToolCallTracker::default();
        let mut reasoning_details = OpenRouterReasoningTracker::default();
        let mut cancellation_open = true;

        assert_eq!(
            process_event(
                event.as_bytes(),
                &sink,
                &mut usage,
                &mut state,
                &mut tool_calls,
                &mut reasoning_details,
                OpenAiCompatibleDialect::OpenRouter,
                true,
                &mut cancelled,
                &mut cancellation_open,
            )
            .await
            .expect("valid OpenRouter detail chunk"),
            EventAction::Continue
        );
        assert_eq!(usage.input_tokens, Some(17));
        assert_eq!(usage.cached_read_tokens, Some(11));
        assert_eq!(usage.cached_write_tokens, Some(6));
        assert_eq!(usage.output_tokens, Some(3));
        assert_eq!(state, SseStreamState::Terminal);

        let event = events.try_recv().expect("message topology");
        let expected_details = detail_values
            .iter()
            .map(|value| OpenRouterReasoningDetail::from_value(value).expect("valid detail"))
            .collect::<Vec<_>>();
        assert_eq!(
            event,
            ProviderEvent::OpaqueReasoningState(OpaqueReasoningState::OpenRouterReasoning {
                topology: OpenRouterReasoningTopology::new(None, Some(expected_details))
                    .expect("valid topology"),
            })
        );
        assert!(reasoning_details.details.is_empty());
        assert!(reasoning_details.reasoning.is_none());
        assert!(events.try_recv().is_err());
        let debug = format!("{event:?}");
        assert!(!debug.contains("private-reasoning-canary"));
        assert!(!debug.contains("private-encrypted-canary"));
    }

    fn observe_openrouter_detail(
        tracker: &mut OpenRouterReasoningTracker,
        detail: serde_json::Value,
    ) {
        let details = serde_json::Value::Array(vec![detail]);
        tracker
            .observe(None, Some(&details), true)
            .expect("valid OpenRouter detail fragment");
    }

    #[test]
    fn openrouter_tracker_merges_nullish_text_and_summary_fragments() {
        let mut tracker = OpenRouterReasoningTracker::default();
        tracker
            .observe(Some("plain-"), None, true)
            .expect("first plaintext fragment");
        tracker
            .observe(Some("reasoning"), None, true)
            .expect("second plaintext fragment");
        observe_openrouter_detail(
            &mut tracker,
            serde_json::json!({
                "type": "reasoning.text", "text": null, "id": "text-1",
                "format": "anthropic-claude-v1", "index": 0
            }),
        );
        observe_openrouter_detail(
            &mut tracker,
            serde_json::json!({
                "type": "reasoning.text", "text": "text-",
                "signature": "late-signature", "index": 0
            }),
        );
        observe_openrouter_detail(
            &mut tracker,
            serde_json::json!({"type": "reasoning.text", "text": "fragment", "index": 0}),
        );
        observe_openrouter_detail(
            &mut tracker,
            serde_json::json!({"type": "reasoning.text", "text": null, "index": 0}),
        );
        observe_openrouter_detail(
            &mut tracker,
            serde_json::json!({
                "type": "reasoning.summary", "summary": null, "id": "summary-1",
                "format": "anthropic-claude-v1", "index": 1
            }),
        );
        observe_openrouter_detail(
            &mut tracker,
            serde_json::json!({"type": "reasoning.summary", "summary": "sum-", "index": 1}),
        );
        observe_openrouter_detail(
            &mut tracker,
            serde_json::json!({"type": "reasoning.summary", "summary": null, "index": 1}),
        );
        observe_openrouter_detail(
            &mut tracker,
            serde_json::json!({"type": "reasoning.summary", "summary": "mary", "index": 1}),
        );

        let events = tracker.finish(false).expect("complete topology");
        let [
            ProviderEvent::OpaqueReasoningState(OpaqueReasoningState::OpenRouterReasoning {
                topology,
            }),
        ] = events.as_slice()
        else {
            panic!("expected one message-level OpenRouter topology");
        };
        assert_eq!(topology.reasoning(), Some("plain-reasoning"));
        let values = topology
            .reasoning_details()
            .expect("details were observed")
            .iter()
            .map(|detail| {
                serde_json::from_str::<serde_json::Value>(detail.expose_to_provider())
                    .expect("canonical detail")
            })
            .collect::<Vec<_>>();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["text"], "text-fragment");
        assert_eq!(values[0]["signature"], "late-signature");
        assert_eq!(values[1]["summary"], "sum-mary");
        assert!(tracker.details.is_empty());
        assert!(tracker.reasoning.is_none());
    }

    #[test]
    fn openrouter_tracker_keeps_empty_marker_and_signature_only_details() {
        let mut empty = OpenRouterReasoningTracker::default();
        empty
            .observe(None, Some(&serde_json::json!([])), true)
            .expect("observed empty details marker");
        let events = empty.finish(false).expect("empty marker topology");
        let [
            ProviderEvent::OpaqueReasoningState(OpaqueReasoningState::OpenRouterReasoning {
                topology,
            }),
        ] = events.as_slice()
        else {
            panic!("expected one empty-marker topology");
        };
        assert_eq!(topology.reasoning_details(), Some([].as_slice()));

        for signature_only in [
            serde_json::json!({
                "type": "reasoning.text",
                "signature": "missing-text-signature"
            }),
            serde_json::json!({
                "type": "reasoning.text",
                "text": null,
                "signature": "null-text-signature",
                "id": null,
                "format": null,
                "index": null
            }),
        ] {
            let mut tracker = OpenRouterReasoningTracker::default();
            tracker
                .observe(
                    None,
                    Some(&serde_json::json!([signature_only.clone()])),
                    true,
                )
                .expect("signature-only fragment");
            let events = tracker.finish(false).expect("signature-only topology");
            let [
                ProviderEvent::OpaqueReasoningState(OpaqueReasoningState::OpenRouterReasoning {
                    topology,
                }),
            ] = events.as_slice()
            else {
                panic!("expected one signature-only topology");
            };
            let [detail] = topology
                .reasoning_details()
                .expect("signature-only details")
            else {
                panic!("expected one signature-only detail");
            };
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(detail.expose_to_provider())
                    .expect("signature-only provider value"),
                signature_only
            );
        }
    }

    #[test]
    fn openrouter_tracker_rejects_conflicting_fragment_metadata_and_wipes_state() {
        for conflicting in [
            serde_json::json!({
                "type": "reasoning.text",
                "text": "second",
                "id": "different-id",
                "format": "anthropic-claude-v1",
                "index": 0
            }),
            serde_json::json!({
                "type": "reasoning.text",
                "text": "second",
                "id": "text-1",
                "format": "different-format",
                "index": 0
            }),
            serde_json::json!({
                "type": "reasoning.text",
                "text": "second",
                "id": "text-1",
                "format": "anthropic-claude-v1",
                "index": 1
            }),
        ] {
            let mut tracker = OpenRouterReasoningTracker::default();
            tracker
                .observe(
                    Some("private-plaintext-canary"),
                    Some(&serde_json::json!([{
                        "type": "reasoning.text",
                        "text": "private-structured-canary",
                        "id": "text-1",
                        "format": "anthropic-claude-v1",
                        "index": 0
                    }])),
                    true,
                )
                .expect("first fragment");
            let error = tracker
                .observe(None, Some(&serde_json::json!([conflicting])), true)
                .expect_err("conflicting metadata must fail closed");
            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
            assert!(!error.message.contains("private-plaintext-canary"));
            assert!(!error.message.contains("private-structured-canary"));
            assert!(tracker.details.is_empty());
            assert!(tracker.reasoning.is_none());
            assert!(!tracker.reasoning_details_observed);
        }
    }

    #[test]
    fn openrouter_tracker_rejects_oversized_detail_and_wipes_state() {
        let canary = "private-oversized-openrouter-canary";
        let mut tracker = OpenRouterReasoningTracker::default();
        let details = serde_json::json!([{
            "type": "reasoning.text",
            "text": format!("{canary}{}", "x".repeat(MAX_OPAQUE_REASONING_ITEM_BYTES)),
            "id": null,
            "format": null
        }]);
        let error = tracker
            .observe(Some("private-plaintext-canary"), Some(&details), true)
            .expect_err("oversized detail must fail closed");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert!(!error.message.contains(canary));
        assert!(!error.message.contains("private-plaintext-canary"));
        assert!(tracker.details.is_empty());
        assert!(tracker.reasoning.is_none());
        assert!(!tracker.reasoning_details_observed);
    }

    #[tokio::test]
    async fn accepts_exactly_one_mib_event_when_separator_is_fragmented() {
        let prefix = b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"";
        let suffix = b"\"}}]}";
        let content_bytes = MAX_SSE_BUFFER_BYTES - prefix.len() - suffix.len();
        for separator in [b"\n\n".as_slice(), b"\r\r".as_slice()] {
            let mut body = Vec::with_capacity(MAX_SSE_BUFFER_BYTES + 32);
            body.extend_from_slice(prefix);
            body.extend(std::iter::repeat_n(b'x', content_bytes));
            body.extend_from_slice(suffix);
            assert_eq!(body.len(), MAX_SSE_BUFFER_BYTES);
            body.extend_from_slice(separator);
            body.extend_from_slice(
                b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}",
            );
            body.extend_from_slice(separator);
            body.extend_from_slice(b"data: [DONE]");
            body.extend_from_slice(separator);

            let (result, events) = generate_from_stream(&body, MAX_SSE_BUFFER_BYTES + 1).await;

            result.expect("event at the bound must succeed");
            assert_eq!(events.len(), 1);
            let ProviderEvent::TextDelta(content) = &events[0] else {
                panic!("expected text delta");
            };
            assert_eq!(content.len(), content_bytes);
        }
    }

    #[tokio::test]
    async fn retains_one_mib_streaming_event_bound() {
        let mut body = b"data: ".to_vec();
        body.extend(std::iter::repeat_n(
            b'x',
            MAX_SSE_BUFFER_BYTES + 1 - body.len(),
        ));

        let (result, events) =
            generate_from_stream_with_timeout(&body, 64 * 1024, Duration::from_secs(30)).await;
        let error = result.expect_err("oversized event must fail");

        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(error.message, "provider streaming event exceeded 1 MiB");
        assert!(events.is_empty());
    }

    #[test]
    fn eof_rejects_oversized_event_ending_in_a_separator_prefix() {
        for suffix in [b'\r', b'\n'] {
            let mut pending = vec![b'x'; MAX_SSE_BUFFER_BYTES];
            pending.push(suffix);

            ensure_pending_size(&pending, false)
                .expect("a separator prefix may need one more network chunk");
            let error = ensure_pending_size(&pending, true)
                .expect_err("EOF resolves the extra byte as oversized event data");

            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
            assert_eq!(error.message, "provider streaming event exceeded 1 MiB");
        }
        assert!(ensure_stream_size(MAX_STREAM_BYTES).is_ok());
        assert_eq!(
            ensure_stream_size(MAX_STREAM_BYTES + 1)
                .expect_err("oversized stream")
                .message,
            "provider stream exceeded 64 MiB"
        );
    }

    #[tokio::test]
    async fn cancellation_interrupts_events_buffered_in_one_http_chunk() {
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"first\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"second\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let provider = OpenAiCompatibleProvider::new(
            &stream_server(body.as_bytes(), body.len()),
            Duration::from_secs(2),
        )
        .expect("provider");
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
        .expect("first buffered event");
        cancel.send(true).expect("cancel");

        let error = generation
            .await
            .expect("generation task")
            .expect_err("cancellation must interrupt buffered event draining");
        assert_eq!(error.code, CoreErrorCode::Cancelled);
        assert_eq!(
            events.try_recv().expect("first event remains buffered"),
            ProviderEvent::TextDelta("first".to_owned())
        );
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn cancellation_is_observed_before_connecting() {
        let provider =
            OpenAiCompatibleProvider::new("http://127.0.0.1:9/v1", Duration::from_secs(2))
                .expect("provider");
        let (sink, _events) = mpsc::channel(4);
        let (cancel, cancelled) = watch::channel(false);
        cancel.send(true).expect("cancel");
        let error = provider
            .generate(request(), None, sink, cancelled)
            .await
            .expect_err("cancelled request");
        assert_eq!(error.code, CoreErrorCode::Cancelled);
    }
}
