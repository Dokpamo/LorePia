use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;
use lorepia_domain::{
    ApiFamily, AuthBinding, CoreError, CoreErrorCode, CoreResult, GenerationRequest,
    GenerationUsage, MessageRole, ProviderCapabilities, ToolCallArgumentsDelta, ToolCallId,
    ToolName,
};
use reqwest::{
    Response, StatusCode,
    header::{ACCEPT, CONTENT_TYPE},
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use url::Url;

use crate::{
    Provider, ProviderEvent, ProviderEventSender,
    discovery::contains_credential_like_token,
    merge_usage_summary,
    network_transport::{ProviderHttpTarget, authorize_request, validate_credential_for_auth},
    parameter_mapping::{
        GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR, PromptCacheWireDialect, ProviderRequestPlan,
    },
    prompt_contract::ProviderPromptAdapterContract,
    request_plan::planned_json_payload,
};

const MAX_SSE_EVENT_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_EMITTED_TEXT_BYTES: usize = 16 * 1024 * 1024;
const MAX_MODEL_ID_BYTES: usize = 256;
const MAX_GEMINI_REQUEST_MESSAGES: usize = 16 * 1024;
const MAX_GEMINI_RESPONSE_PARTS: usize = 4 * 1024;
const MAX_GEMINI_TOOL_CALLS: u32 = 128;
const MAX_THOUGHT_SIGNATURE_BYTES: usize = 64 * 1024;
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

/// Selects the standard or SSE form of Gemini's Generate Content API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiResponseMode {
    Streaming,
    Unary,
}

/// Adapter for Gemini's native `generateContent` API family.
///
/// The default constructor uses `streamGenerateContent` with SSE. API keys are
/// sent only through the sensitive `x-goog-api-key` header.
#[derive(Clone)]
pub struct GeminiGenerateContentProvider {
    api_base: Url,
    target: ProviderHttpTarget,
    auth: AuthBinding,
    api_base_is_model_collection: bool,
    mode: GeminiResponseMode,
    include_thought_summaries: bool,
    request_plan: Option<ProviderRequestPlan>,
}

impl GeminiGenerateContentProvider {
    pub fn new(base_url: &str, timeout: Duration) -> CoreResult<Self> {
        Self::with_mode(base_url, timeout, GeminiResponseMode::Streaming)
    }

    pub fn new_unary(base_url: &str, timeout: Duration) -> CoreResult<Self> {
        Self::with_mode(base_url, timeout, GeminiResponseMode::Unary)
    }

    pub fn with_mode(
        base_url: &str,
        timeout: Duration,
        mode: GeminiResponseMode,
    ) -> CoreResult<Self> {
        let mut api_base =
            Url::parse(base_url).map_err(|_| CoreError::invalid("invalid Gemini API base URL"))?;
        validate_api_base(&api_base)?;
        if !api_base.path().ends_with('/') {
            let path = format!("{}/", api_base.path());
            api_base.set_path(&path);
        }
        let target = ProviderHttpTarget::inferred(api_base.as_str(), timeout)?;
        Ok(Self {
            api_base,
            target,
            auth: AuthBinding::HeaderApiKey {
                header_name: lorepia_domain::HeaderName::parse("x-goog-api-key")
                    .map_err(CoreError::internal)?,
            },
            api_base_is_model_collection: false,
            mode,
            include_thought_summaries: true,
            request_plan: None,
        })
    }

    pub(crate) fn new_with_manifest_target(
        target: ProviderHttpTarget,
        auth: AuthBinding,
        mode: GeminiResponseMode,
    ) -> Self {
        Self {
            api_base: target.url().clone(),
            target,
            auth,
            api_base_is_model_collection: true,
            mode,
            include_thought_summaries: true,
            request_plan: None,
        }
    }

    /// Controls whether Gemini should return its supported thought summaries.
    ///
    /// The adapter never treats an encrypted thought signature as reasoning;
    /// only text parts explicitly marked `thought: true` are emitted as such.
    #[must_use]
    pub fn with_thought_summaries(mut self, include: bool) -> Self {
        self.include_thought_summaries = include;
        self
    }

    #[must_use]
    pub fn with_request_plan(mut self, plan: ProviderRequestPlan) -> Self {
        self.request_plan = Some(plan);
        self.include_thought_summaries = false;
        self
    }

    pub(crate) fn with_optional_request_plan(mut self, plan: Option<ProviderRequestPlan>) -> Self {
        self.include_thought_summaries = plan.is_none();
        self.request_plan = plan;
        self
    }

    fn endpoint(&self, model: &str) -> CoreResult<Url> {
        let model = validate_model_id(model)?;
        let method = match self.mode {
            GeminiResponseMode::Streaming => format!("{model}:streamGenerateContent"),
            GeminiResponseMode::Unary => format!("{model}:generateContent"),
        };
        let mut endpoint = self.api_base.clone();
        endpoint
            .path_segments_mut()
            .map_err(|()| CoreError::invalid("Gemini API base URL cannot be extended"))?
            .pop_if_empty()
            .extend(if self.api_base_is_model_collection {
                vec![method.as_str()]
            } else {
                vec!["models", method.as_str()]
            });
        if self.mode == GeminiResponseMode::Streaming {
            endpoint.query_pairs_mut().append_pair("alt", "sse");
        }
        Ok(endpoint)
    }
}

#[async_trait]
impl Provider for GeminiGenerateContentProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: self.mode == GeminiResponseMode::Streaming,
            reasoning: self.include_thought_summaries,
            max_context_tokens: None,
        }
    }

    fn snapshot_request(&self, request: &GenerationRequest) -> CoreResult<serde_json::Value> {
        crate::validate_snapshot_request(request)?;
        let cache_dialect = self
            .request_plan
            .as_ref()
            .map_or(PromptCacheWireDialect::Unsupported, |plan| {
                plan.cache_dialect()
            });
        let payload = request_payload_with_cache(
            request.clone(),
            self.include_thought_summaries,
            cache_dialect,
        )?;
        planned_json_payload(
            &payload,
            ApiFamily::GeminiGenerateContent,
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
        let endpoint = self.endpoint(&request.model)?;
        let cache_dialect = self
            .request_plan
            .as_ref()
            .map_or(PromptCacheWireDialect::Unsupported, |plan| {
                plan.cache_dialect()
            });
        let payload =
            request_payload_with_cache(request, self.include_thought_summaries, cache_dialect)?;
        let payload = planned_json_payload(
            &payload,
            ApiFamily::GeminiGenerateContent,
            self.request_plan.as_ref(),
        )?;
        validate_credential_for_auth(&self.auth, credential)?;
        let prepared = self.target.prepare().await?;
        ensure_not_cancelled(&cancelled)?;
        let mut response_future = Box::pin(
            authorize_request(
                prepared
                    .client()
                    .post(endpoint)
                    .header(ACCEPT, response_accept(self.mode))
                    .json(&payload),
                &self.auth,
                credential,
            )?
            .send(),
        );
        let mut cancellation_open = true;
        let response = loop {
            tokio::select! {
                biased;
                change = cancelled.changed(), if cancellation_open => {
                    if change.is_err() {
                        cancellation_open = false;
                    } else {
                        ensure_not_cancelled(&cancelled)?;
                    }
                }
                result = &mut response_future => {
                    break result.map_err(network_error)?;
                }
            }
        };
        prepared.validate_response_peer(&response)?;
        validate_response_status(&response)?;
        validate_declared_response_size(&response)?;

        match self.mode {
            GeminiResponseMode::Streaming => {
                validate_stream_content_type(&response)?;
                consume_sse(response, &sink, &mut cancelled, &mut cancellation_open).await
            }
            GeminiResponseMode::Unary => {
                validate_unary_content_type(&response)?;
                consume_unary(response, &sink, &mut cancelled, &mut cancellation_open).await
            }
        }
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

fn response_accept(mode: GeminiResponseMode) -> &'static str {
    match mode {
        GeminiResponseMode::Streaming => "text/event-stream",
        GeminiResponseMode::Unary => "application/json",
    }
}

#[cfg(test)]
fn request_payload(
    request: GenerationRequest,
    include_thought_summaries: bool,
) -> CoreResult<GenerateContentRequest> {
    request_payload_with_cache(
        request,
        include_thought_summaries,
        PromptCacheWireDialect::Unsupported,
    )
}

fn request_payload_with_cache(
    request: GenerationRequest,
    include_thought_summaries: bool,
    cache_dialect: PromptCacheWireDialect,
) -> CoreResult<GenerateContentRequest> {
    validate_gemini_request(&request)?;
    let (system_parts, contents) = materialize_gemini_prompt(&request, cache_dialect)?;

    let thinking_config = include_thought_summaries.then_some(ThinkingConfig {
        include_thoughts: true,
    });
    let generation_config = (request.temperature.is_some()
        || request.max_output_tokens.is_some()
        || thinking_config.is_some())
    .then_some(GenerationConfig {
        temperature: request.temperature,
        max_output_tokens: request.max_output_tokens,
        thinking_config,
    });
    Ok(GenerateContentRequest {
        system_instruction: (!system_parts.is_empty()).then_some(Content {
            role: None,
            parts: system_parts,
        }),
        contents,
        generation_config,
    })
}

fn validate_gemini_request(request: &GenerationRequest) -> CoreResult<()> {
    if request.resolved_prompt_plan.is_none() && request.provider_execution_plan_hash.is_some() {
        return Err(CoreError::invalid(
            "provider execution plan hash requires a resolved prompt plan",
        ));
    }
    if request
        .temperature
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(CoreError::invalid(
            "Gemini temperature must be a non-negative finite number",
        ));
    }
    if request.max_output_tokens == Some(0) {
        return Err(CoreError::invalid(
            "Gemini max output tokens must be greater than zero",
        ));
    }
    let prompt_message_count = request
        .resolved_prompt_plan
        .as_ref()
        .map_or(request.messages.len(), |plan| plan.effective_messages.len());
    if prompt_message_count > MAX_GEMINI_REQUEST_MESSAGES {
        return Err(CoreError::invalid(
            "Gemini request contains too many messages",
        ));
    }
    if request.preserve_opaque_reasoning_state || !request.opaque_reasoning_context.is_empty() {
        // Google requires a signature to be returned on the exact original
        // Part. LorePia currently persists flattened assistant text rather
        // than the complete private Gemini Content/Part topology, so it cannot
        // safely capture state now and surprise-fail only when the next turn
        // tries to replay it. Never merge signed and unsigned Parts, synthesize
        // placeholder Parts, or silently ignore supplied continuity context.
        return Err(CoreError::invalid(GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR));
    }
    Ok(())
}

fn materialize_gemini_prompt(
    request: &GenerationRequest,
    cache_dialect: PromptCacheWireDialect,
) -> CoreResult<(Vec<TextPart>, Vec<Content>)> {
    let mut system_parts = Vec::new();
    let mut contents = Vec::new();
    let mut request_text_bytes = 0_usize;
    if let Some(plan) = &request.resolved_prompt_plan {
        let compiled = prompt_contract()
            .compile_resolved_plan_for_execution(plan, cache_dialect)
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        compiled
            .verify_execution_hash(request.provider_execution_plan_hash.as_deref())
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        for message in compiled.messages() {
            request_text_bytes = request_text_bytes
                .checked_add(message.content().len())
                .ok_or_else(|| CoreError::invalid("Gemini request messages are too large"))?;
            if request_text_bytes > MAX_RESPONSE_BYTES {
                return Err(CoreError::invalid("Gemini request message is too large"));
            }
            match message.placement() {
                crate::prompt_contract::ProviderPromptPlacement::SystemInstruction => {
                    system_parts.push(TextPart {
                        text: message.content().to_owned(),
                    });
                }
                crate::prompt_contract::ProviderPromptPlacement::Message => {
                    contents.push(Content {
                        role: Some(message.wire_role().as_str()),
                        parts: vec![TextPart {
                            text: message.content().to_owned(),
                        }],
                    });
                }
            }
        }
    } else {
        for message in &request.messages {
            request_text_bytes = request_text_bytes
                .checked_add(message.content.len())
                .ok_or_else(|| CoreError::invalid("Gemini request messages are too large"))?;
            if request_text_bytes > MAX_RESPONSE_BYTES {
                return Err(CoreError::invalid("Gemini request message is too large"));
            }
            match message.role {
                MessageRole::System => {
                    debug_assert_eq!(
                        prompt_contract()
                            .map_message_role(MessageRole::System)
                            .wire_role
                            .as_str(),
                        "system"
                    );
                    system_parts.push(TextPart {
                        text: message.content.clone(),
                    });
                }
                MessageRole::User | MessageRole::Assistant => {
                    contents.push(Content {
                        role: Some(role_name(message.role)),
                        parts: vec![TextPart {
                            text: message.content.clone(),
                        }],
                    });
                }
            }
        }
    }
    if contents.is_empty() {
        return Err(CoreError::invalid(
            "Gemini generation requires at least one user or model message",
        ));
    }
    Ok((system_parts, contents))
}

pub(crate) const fn prompt_contract() -> ProviderPromptAdapterContract {
    ProviderPromptAdapterContract::for_family(ApiFamily::GeminiGenerateContent)
}

#[cfg(test)]
pub(crate) fn resolved_prompt_payload_for_contract_test(
    request: GenerationRequest,
    cache_dialect: PromptCacheWireDialect,
) -> CoreResult<serde_json::Value> {
    let payload = request_payload_with_cache(request, false, cache_dialect)?;
    serde_json::to_value(payload)
        .map_err(|_| CoreError::internal("failed to serialize Gemini test payload"))
}

fn role_name(role: MessageRole) -> &'static str {
    prompt_contract().map_message_role(role).wire_role.as_str()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<Content>,
    contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

#[derive(Serialize)]
struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
    parts: Vec<TextPart>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TextPart {
    text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_config: Option<ThinkingConfig>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThinkingConfig {
    include_thoughts: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    prompt_feedback: Option<PromptFeedback>,
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    content: Option<ResponseContent>,
    finish_reason: Option<String>,
    index: Option<u32>,
}

#[derive(Default, Deserialize)]
struct ResponseContent {
    role: Option<String>,
    #[serde(default)]
    parts: Vec<ResponsePart>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResponsePart {
    text: Option<String>,
    #[serde(default)]
    thought: bool,
    thought_signature: Option<String>,
    function_call: Option<GeminiFunctionCall>,
    #[serde(flatten)]
    other: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct GeminiFunctionCall {
    id: Option<String>,
    name: Option<String>,
    args: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptFeedback {
    block_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
struct UsageMetadata {
    prompt_token_count: Option<u64>,
    cached_content_token_count: Option<u64>,
    candidates_token_count: Option<u64>,
    tool_use_prompt_token_count: Option<u64>,
    thoughts_token_count: Option<u64>,
    total_token_count: Option<u64>,
}

#[derive(Debug, Default)]
struct ResponseState {
    progress: ResponseProgress,
    emitted_text_bytes: usize,
    response_parts: usize,
    next_tool_call_index: u32,
    tool_call_ids: BTreeSet<ToolCallId>,
    usage: GenerationUsage,
}

#[derive(Debug, Default)]
enum ResponseProgress {
    #[default]
    Empty,
    ResponseOnly,
    Candidate,
    SupportedContent,
    TerminalWithoutContent,
    TerminalWithContent,
}

impl ResponseProgress {
    fn observe_response(&mut self) {
        if matches!(self, Self::Empty) {
            *self = Self::ResponseOnly;
        }
    }

    fn observe_candidate(&mut self) {
        if matches!(self, Self::Empty | Self::ResponseOnly) {
            *self = Self::Candidate;
        }
    }

    fn observe_supported_content(&mut self) {
        *self = Self::SupportedContent;
    }

    fn observe_terminal(&mut self) {
        *self = if matches!(self, Self::SupportedContent) {
            Self::TerminalWithContent
        } else {
            Self::TerminalWithoutContent
        };
    }

    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::TerminalWithoutContent | Self::TerminalWithContent
        )
    }
}

impl ResponseState {
    fn finish(self) -> CoreResult<GenerationUsage> {
        match self.progress {
            ResponseProgress::Empty => Err(streaming_error("Gemini returned an empty response")),
            ResponseProgress::ResponseOnly => Err(streaming_error(
                "Gemini response did not contain a candidate",
            )),
            ResponseProgress::Candidate | ResponseProgress::TerminalWithoutContent => Err(
                streaming_error("Gemini response did not contain a supported content part"),
            ),
            ResponseProgress::SupportedContent => Err(streaming_error(
                "Gemini response ended without a finish reason",
            )),
            ResponseProgress::TerminalWithContent => Ok(self.usage),
        }
    }
}

async fn consume_sse(
    response: Response,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<GenerationUsage> {
    let mut bytes = response.bytes_stream();
    let mut pending = Vec::<u8>::new();
    let mut response_bytes = 0_usize;
    let mut state = ResponseState::default();
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
            chunk = bytes.next() => {
                let Some(chunk) = chunk else { break };
                let chunk = chunk.map_err(network_error)?;
                response_bytes = response_bytes.checked_add(chunk.len())
                    .ok_or_else(response_too_large)?;
                if response_bytes > MAX_RESPONSE_BYTES {
                    return Err(response_too_large());
                }
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
                        process_sse_event(
                            &event,
                            sink,
                            cancelled,
                            cancellation_open,
                            &mut state,
                        )
                        .await?;
                    }
                }
                ensure_pending_size(&pending, false)?;
            }
        }
    }
    ensure_not_cancelled(cancelled)?;
    while let Some((boundary, separator_len)) = find_event_boundary(&pending, true) {
        ensure_event_size(boundary)?;
        let event = pending.drain(..boundary).collect::<Vec<_>>();
        pending.drain(..separator_len);
        process_sse_event(&event, sink, cancelled, cancellation_open, &mut state).await?;
    }
    ensure_pending_size(&pending, true)?;
    if !pending.is_empty() {
        return Err(streaming_error(
            "Gemini stream ended with an incomplete SSE event",
        ));
    }
    state.finish()
}

async fn consume_unary(
    response: Response,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<GenerationUsage> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
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
                if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    return Err(response_too_large());
                }
                body.extend_from_slice(&chunk);
            }
        }
    }
    ensure_not_cancelled(cancelled)?;
    if body.is_empty() {
        return Err(streaming_error("Gemini returned an empty response"));
    }
    let response = parse_response(&body)?;
    let mut state = ResponseState::default();
    process_response(response, sink, cancelled, cancellation_open, &mut state).await?;
    state.finish()
}

async fn process_sse_event(
    event: &[u8],
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
    state: &mut ResponseState,
) -> CoreResult<()> {
    let text = std::str::from_utf8(event)
        .map_err(|_| streaming_error("Gemini returned malformed SSE data"))?;
    let mut data_lines = Vec::new();
    for line in text.split(['\r', '\n']) {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        data_lines.push(data.strip_prefix(' ').unwrap_or(data));
    }
    if data_lines.is_empty() {
        return Ok(());
    }
    let data = data_lines.join("\n");
    let data = data.trim();
    if data.is_empty() {
        return Ok(());
    }
    if data == "[DONE]" {
        return Err(streaming_error(
            "Gemini returned an unsupported SSE terminal marker",
        ));
    }
    let response = parse_response(data.as_bytes())?;
    process_response(response, sink, cancelled, cancellation_open, state).await
}

fn parse_response(bytes: &[u8]) -> CoreResult<GenerateContentResponse> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| streaming_error("Gemini returned malformed response data"))?;
    let Some(object) = value.as_object() else {
        return Err(streaming_error("Gemini returned malformed response data"));
    };
    if object
        .get("error")
        .is_some_and(|provider_error| !provider_error.is_null())
    {
        return Err(streaming_error("Gemini returned a response error"));
    }
    if !object.contains_key("candidates")
        && !object.contains_key("usageMetadata")
        && !object.contains_key("promptFeedback")
    {
        return Err(streaming_error("Gemini returned malformed response data"));
    }
    serde_json::from_value(value)
        .map_err(|_| streaming_error("Gemini returned malformed response data"))
}

async fn process_response(
    response: GenerateContentResponse,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
    state: &mut ResponseState,
) -> CoreResult<()> {
    state.progress.observe_response();
    if response
        .prompt_feedback
        .as_ref()
        .and_then(|feedback| feedback.block_reason.as_deref())
        .is_some()
    {
        return Err(CoreError::new(
            CoreErrorCode::ProviderUnavailable,
            "Gemini blocked the prompt",
            false,
        ));
    }
    update_usage(response.usage_metadata.as_ref(), &mut state.usage);
    if response.candidates.len() > 1 {
        return Err(streaming_error(
            "Gemini returned multiple response candidates",
        ));
    }
    let Some(candidate) = response.candidates.into_iter().next() else {
        return Ok(());
    };
    if candidate.index.is_some_and(|index| index != 0) {
        return Err(streaming_error(
            "Gemini returned an unexpected candidate index",
        ));
    }
    if state.progress.is_terminal() {
        return Err(streaming_error(
            "Gemini returned candidate data after a finish reason",
        ));
    }
    state.progress.observe_candidate();
    if let Some(content) = candidate.content {
        if content.role.as_deref().is_some_and(|role| role != "model") {
            return Err(streaming_error(
                "Gemini returned content with an unexpected role",
            ));
        }
        for part in content.parts {
            process_response_part(part, state, sink, cancelled, cancellation_open).await?;
        }
    }
    if let Some(reason) = candidate.finish_reason {
        observe_finish_reason(&reason)?;
        state.progress.observe_terminal();
    }
    Ok(())
}

async fn process_response_part(
    part: ResponsePart,
    state: &mut ResponseState,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<()> {
    state.response_parts = state
        .response_parts
        .checked_add(1)
        .filter(|count| *count <= MAX_GEMINI_RESPONSE_PARTS)
        .ok_or_else(|| streaming_error("Gemini returned too many response parts"))?;
    if !part.other.is_empty() {
        return Err(streaming_error(
            "Gemini returned an unsupported response part",
        ));
    }
    if part.text.is_none() && part.function_call.is_none() {
        return Err(streaming_error(
            "Gemini returned a response part without supported content",
        ));
    }
    if part.text.is_some() && part.function_call.is_some() {
        return Err(streaming_error(
            "Gemini returned conflicting response part content",
        ));
    }
    if part.thought && part.function_call.is_some() {
        return Err(streaming_error(
            "Gemini returned a thought marker on a function-call part",
        ));
    }
    state.progress.observe_supported_content();
    validate_thought_signature(part.thought_signature.as_deref())?;
    if let Some(function_call) = part.function_call {
        return emit_gemini_function_call(function_call, state, sink, cancelled, cancellation_open)
            .await;
    }
    if let Some(text) = part.text.filter(|text| !text.is_empty()) {
        state.emitted_text_bytes = state
            .emitted_text_bytes
            .checked_add(text.len())
            .ok_or_else(response_too_large)?;
        if state.emitted_text_bytes > MAX_EMITTED_TEXT_BYTES {
            return Err(response_too_large());
        }
        let event = if part.thought {
            ProviderEvent::ReasoningDelta(text)
        } else {
            ProviderEvent::TextDelta(text)
        };
        send_provider_event(sink, event, cancelled, cancellation_open).await?;
    }
    Ok(())
}

fn validate_thought_signature(signature: Option<&str>) -> CoreResult<()> {
    let Some(signature) = signature else {
        return Ok(());
    };
    if signature.len() > MAX_THOUGHT_SIGNATURE_BYTES {
        return Err(streaming_error(
            "Gemini thought signature exceeded its safety limit",
        ));
    }
    let decoded_signature = BASE64
        .decode(signature)
        .map_err(|_| streaming_error("Gemini returned malformed opaque reasoning state"))?;
    if BASE64.encode(&decoded_signature) != signature {
        return Err(streaming_error(
            "Gemini returned malformed opaque reasoning state",
        ));
    }
    if decoded_signature.is_empty() {
        return Err(streaming_error(
            "Gemini returned malformed opaque reasoning state",
        ));
    }
    Ok(())
}

async fn emit_gemini_function_call(
    function_call: GeminiFunctionCall,
    state: &mut ResponseState,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<()> {
    let index = state.next_tool_call_index;
    state.next_tool_call_index = state
        .next_tool_call_index
        .checked_add(1)
        .filter(|count| *count <= MAX_GEMINI_TOOL_CALLS)
        .ok_or_else(|| streaming_error("Gemini returned too many tool calls"))?;
    let id = match function_call.id {
        Some(id) => ToolCallId::parse(id),
        None => ToolCallId::parse(format!("gemini-call-{index}")),
    }
    .map_err(|_| streaming_error("Gemini returned an invalid tool-call id"))?;
    if !state.tool_call_ids.insert(id.clone()) {
        return Err(streaming_error("Gemini reused a tool-call identifier"));
    }
    let name = function_call
        .name
        .ok_or_else(|| streaming_error("Gemini returned a tool call without a name"))
        .and_then(|name| {
            ToolName::parse(name)
                .map_err(|_| streaming_error("Gemini returned an invalid tool name"))
        })?;
    let arguments = function_call
        .args
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    if !arguments.is_object() {
        return Err(streaming_error(
            "Gemini returned non-object tool-call arguments",
        ));
    }
    let arguments = serde_json::to_string(&arguments)
        .map_err(|_| streaming_error("Gemini returned invalid tool-call arguments"))
        .and_then(|arguments| {
            ToolCallArgumentsDelta::parse(arguments).map_err(|_| {
                streaming_error("Gemini tool-call arguments exceeded its safety limit")
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

fn update_usage(metadata: Option<&UsageMetadata>, usage: &mut GenerationUsage) {
    if let Some(metadata) = metadata {
        usage.input_tokens = metadata.prompt_token_count.or(usage.input_tokens);
        // `candidatesTokenCount` is Google's documented generated response count.
        usage.output_tokens = metadata.candidates_token_count.or(usage.output_tokens);
        usage.cached_read_tokens = metadata
            .cached_content_token_count
            .or(usage.cached_read_tokens);
        usage.reasoning_tokens = metadata.thoughts_token_count.or(usage.reasoning_tokens);
        usage.tool_tokens = metadata.tool_use_prompt_token_count.or(usage.tool_tokens);
        usage.provider_raw_summary = merge_usage_summary(
            usage.provider_raw_summary.as_ref(),
            &[("totalTokenCount", metadata.total_token_count)],
        );
    }
}

fn observe_finish_reason(reason: &str) -> CoreResult<()> {
    match reason {
        "STOP" | "MAX_TOKENS" => Ok(()),
        "SAFETY"
        | "RECITATION"
        | "LANGUAGE"
        | "BLOCKLIST"
        | "PROHIBITED_CONTENT"
        | "SPII"
        | "IMAGE_SAFETY"
        | "IMAGE_PROHIBITED_CONTENT"
        | "IMAGE_OTHER"
        | "NO_IMAGE"
        | "IMAGE_RECITATION"
        | "ESCALATION" => Err(CoreError::new(
            CoreErrorCode::ProviderUnavailable,
            "Gemini stopped generation due to provider policy",
            false,
        )),
        "MALFORMED_FUNCTION_CALL"
        | "UNEXPECTED_TOOL_CALL"
        | "TOO_MANY_TOOL_CALLS"
        | "MISSING_THOUGHT_SIGNATURE"
        | "MALFORMED_RESPONSE" => Err(streaming_error(
            "Gemini could not produce a supported response",
        )),
        "OTHER" => Err(streaming_error(
            "Gemini stopped generation without a supported result",
        )),
        _ => Err(streaming_error(
            "Gemini returned an unsupported finish reason",
        )),
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

fn validate_api_base(base: &Url) -> CoreResult<()> {
    if !base.username().is_empty() || base.password().is_some() {
        return Err(CoreError::invalid(
            "Gemini API base URL must not contain embedded credentials",
        ));
    }
    if base.query().is_some() || base.fragment().is_some() {
        return Err(CoreError::invalid(
            "Gemini API base URL must not contain a query or fragment",
        ));
    }
    match base.scheme() {
        "https" => Ok(()),
        "http" if base.host_str().is_some_and(is_loopback_host) => Ok(()),
        "http" => Err(CoreError::invalid(
            "unencrypted HTTP is allowed only for loopback Gemini endpoints",
        )),
        _ => Err(CoreError::invalid(
            "Gemini API base URL must use HTTPS or loopback HTTP",
        )),
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn validate_model_id(model: &str) -> CoreResult<&str> {
    let model = model.strip_prefix("models/").unwrap_or(model);
    if model.is_empty() || model.len() > MAX_MODEL_ID_BYTES || contains_credential_like_token(model)
    {
        return Err(CoreError::invalid("invalid Gemini model ID"));
    }
    if !model
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || matches!(model, "." | "..")
    {
        return Err(CoreError::invalid("invalid Gemini model ID"));
    }
    Ok(model)
}

fn validate_response_status(response: &Response) -> CoreResult<()> {
    if response.status().is_success() {
        Ok(())
    } else {
        Err(status_error(response.status()))
    }
}

fn validate_declared_response_size(response: &Response) -> CoreResult<()> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        Err(response_too_large())
    } else {
        Ok(())
    }
}

fn validate_stream_content_type(response: &Response) -> CoreResult<()> {
    let is_sse = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        });
    if is_sse {
        Ok(())
    } else {
        Err(streaming_error(
            "Gemini streaming response was not server-sent events",
        ))
    }
}

fn validate_unary_content_type(response: &Response) -> CoreResult<()> {
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').next().is_some_and(|mime| {
                let mime = mime.trim();
                mime.eq_ignore_ascii_case("application/json")
                    || mime
                        .to_ascii_lowercase()
                        .strip_prefix("application/")
                        .is_some_and(|subtype| subtype.ends_with("+json"))
            })
        });
    if is_json {
        Ok(())
    } else {
        Err(streaming_error(
            "Gemini unary response was not JSON content",
        ))
    }
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
        Err(streaming_error("Gemini SSE event exceeded 1 MiB"))
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
    Err(streaming_error("Gemini SSE event exceeded 1 MiB"))
}

fn network_error(error: reqwest::Error) -> CoreError {
    if error.is_timeout() {
        CoreError::new(
            CoreErrorCode::ProviderUnavailable,
            "Gemini request timed out",
            true,
        )
    } else {
        CoreError::new(
            CoreErrorCode::NetworkUnavailable,
            "Gemini network request failed",
            true,
        )
    }
}

fn status_error(status: StatusCode) -> CoreError {
    let (code, recoverable) = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            (CoreErrorCode::ProviderAuthFailed, false)
        }
        StatusCode::TOO_MANY_REQUESTS => (CoreErrorCode::ProviderRateLimited, true),
        _ => (
            CoreErrorCode::ProviderUnavailable,
            status.is_server_error()
                || status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::TOO_EARLY,
        ),
    };
    CoreError::new(
        code,
        format!("Gemini returned HTTP {}", status.as_u16()),
        recoverable,
    )
}

fn streaming_error(message: &'static str) -> CoreError {
    CoreError::new(CoreErrorCode::ProviderUnavailable, message, true)
}

fn response_too_large() -> CoreError {
    streaming_error("Gemini response exceeded the configured size limit")
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
        GenerationRequest, Message, ModelRouteId, OpaqueReasoningContext, OpaqueReasoningData,
        OpaqueReasoningState,
    };
    use tokio::sync::{mpsc, watch};

    use super::*;

    fn request() -> GenerationRequest {
        GenerationRequest {
            generation_id: GenerationId::new(),
            conversation_id: ConversationId::new(),
            model: "gemini-2.5-flash".to_owned(),
            messages: vec![
                message(MessageRole::System, "be concise"),
                message(MessageRole::User, "hello"),
                message(MessageRole::Assistant, "hi"),
                message(MessageRole::User, "continue"),
            ],
            resolved_prompt_plan: None,
            provider_execution_plan_hash: None,
            temperature: Some(0.7),
            max_output_tokens: Some(128),
            provider_provenance: None,
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        }
    }

    fn message(role: MessageRole, content: &str) -> Message {
        let mut message = Message::user(ConversationId::new(), content);
        message.role = role;
        message
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
                    "request ended before headers",
                ));
            }
            request.extend_from_slice(&buffer[..read]);
            if request.len() > MAX_REQUEST_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "request exceeded test limit",
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
                    "request ended before body",
                ));
            }
            request.extend_from_slice(&buffer[..read]);
        }
        Ok(request)
    }

    fn deferred_server(
        response_headers: &str,
        body: &[u8],
        fragment: usize,
    ) -> (
        String,
        std::sync::mpsc::Receiver<Vec<u8>>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let headers = response_headers.to_owned();
        let body = body.to_vec();
        let (captured_sender, captured_receiver) = std::sync::mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let request = read_http_request(&mut stream).expect("read request");
            captured_sender.send(request).expect("capture request");
            write!(
                stream,
                "HTTP/1.1 {headers}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            )
            .expect("write headers");
            for chunk in body.chunks(fragment.max(1)) {
                if write!(stream, "{:X}\r\n", chunk.len()).is_err()
                    || stream.write_all(chunk).is_err()
                    || stream.write_all(b"\r\n").is_err()
                {
                    return;
                }
            }
            let _ = stream.write_all(b"0\r\n\r\n");
        });
        (
            format!("http://{address}/v1beta"),
            captured_receiver,
            handle,
        )
    }

    async fn run(
        mode: GeminiResponseMode,
        body: &[u8],
        content_type: &str,
        fragment: usize,
    ) -> (CoreResult<GenerationUsage>, Vec<ProviderEvent>, Vec<u8>) {
        run_request(request(), mode, body, content_type, fragment).await
    }

    async fn run_request(
        request: GenerationRequest,
        mode: GeminiResponseMode,
        body: &[u8],
        content_type: &str,
        fragment: usize,
    ) -> (CoreResult<GenerationUsage>, Vec<ProviderEvent>, Vec<u8>) {
        let (base_url, captured, handle) = deferred_server(
            &format!("200 OK\r\nContent-Type: {content_type}"),
            body,
            fragment,
        );
        let provider =
            GeminiGenerateContentProvider::with_mode(&base_url, Duration::from_secs(2), mode)
                .expect("provider");
        let (sink, mut receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let result = provider
            .generate(request, Some("synthetic-secret"), sink, cancelled)
            .await;
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        let captured = captured.recv().expect("captured request");
        handle.join().expect("server");
        (result, events, captured)
    }

    #[test]
    fn validates_transport_and_model_path() {
        assert!(
            GeminiGenerateContentProvider::new("http://example.com/v1beta", Duration::from_secs(1))
                .is_err()
        );
        assert!(
            GeminiGenerateContentProvider::new(
                "https://user:secret@example.com/v1beta",
                Duration::from_secs(1)
            )
            .is_err()
        );
        assert!(
            GeminiGenerateContentProvider::new(
                "https://example.com/v1beta?key=secret",
                Duration::from_secs(1)
            )
            .is_err()
        );
        let provider =
            GeminiGenerateContentProvider::new("http://127.0.0.2:9/v1beta", Duration::from_secs(1))
                .expect("loopback");
        let endpoint = provider
            .endpoint("models/gemini-2.5-flash")
            .expect("endpoint");
        assert_eq!(
            endpoint.as_str(),
            "http://127.0.0.2:9/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
        for invalid in [
            "",
            "models/",
            "../secret",
            "a/b",
            "a%2Fb",
            "a?key=x",
            "한글",
            "sk-reflected-fixture-not-a-real-key",
        ] {
            assert!(provider.endpoint(invalid).is_err(), "{invalid}");
        }
    }

    #[tokio::test]
    async fn streams_text_thoughts_usage_and_uses_header_auth() {
        let body = concat!(
            ": keepalive\r\n\r\n",
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"생각\",\"thought\":true}]},\"index\":0}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"안녕\"}]},\"index\":0}]}\r\n\r\n",
            "data: {\"candidates\":[{\"finishReason\":\"STOP\",\"index\":0}],\"usageMetadata\":{\"promptTokenCount\":4,\"cachedContentTokenCount\":2,\"candidatesTokenCount\":2,\"thoughtsTokenCount\":1,\"toolUsePromptTokenCount\":3,\"totalTokenCount\":10}}\n\n",
        );
        let (result, events, captured) = run(
            GeminiResponseMode::Streaming,
            body.as_bytes(),
            "text/event-stream; charset=utf-8",
            3,
        )
        .await;
        let usage = result.expect("stream");
        assert_eq!(usage.input_tokens, Some(4));
        assert_eq!(usage.cached_read_tokens, Some(2));
        assert_eq!(usage.cached_write_tokens, None);
        assert_eq!(usage.output_tokens, Some(2));
        assert_eq!(usage.reasoning_tokens, Some(1));
        assert_eq!(usage.tool_tokens, Some(3));
        assert_eq!(
            usage
                .provider_raw_summary
                .as_ref()
                .map(lorepia_domain::BoundedJson::as_str),
            Some(r#"{"totalTokenCount":10}"#)
        );
        assert_eq!(
            events,
            vec![
                ProviderEvent::ReasoningDelta("생각".to_owned()),
                ProviderEvent::TextDelta("안녕".to_owned()),
            ]
        );
        let request = String::from_utf8(captured).expect("request text");
        let headers = request.split("\r\n\r\n").next().expect("headers");
        assert!(headers.starts_with(
            "POST /v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse HTTP/1.1"
        ));
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("x-goog-api-key: synthetic-secret")
        );
        assert!(!headers.contains("key=synthetic-secret"));
        let body = request.split_once("\r\n\r\n").expect("body").1;
        assert!(!body.contains("thoughtSignature"));
        let value: serde_json::Value = serde_json::from_str(body).expect("json");
        assert_eq!(value["systemInstruction"]["parts"][0]["text"], "be concise");
        assert_eq!(value["contents"][1]["role"], "model");
        assert_eq!(
            value["generationConfig"]["thinkingConfig"]["includeThoughts"],
            true
        );
    }

    #[tokio::test]
    async fn rejects_opaque_capture_or_context_before_network_access() {
        for context_without_preservation_flag in [false, true] {
            let canary = "unusable-gemini-thought-signature";
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.set_nonblocking(true).expect("nonblocking");
            let base_url = format!("http://{}/v1beta", listener.local_addr().expect("address"));
            let provider = GeminiGenerateContentProvider::new(&base_url, Duration::from_secs(1))
                .expect("provider");
            let mut request = request();
            if context_without_preservation_flag {
                request.opaque_reasoning_context = vec![OpaqueReasoningContext {
                    source_message_id: request.messages[2].id.clone(),
                    api_family: ApiFamily::GeminiGenerateContent,
                    model: request.model.clone(),
                    model_route_id: ModelRouteId::from("gemini-route"),
                    generation_preset_id: GenerationPresetId::from("prior-preset"),
                    state: OpaqueReasoningState::GeminiThoughtSignature {
                        part_index: 0,
                        signature: OpaqueReasoningData::parse(canary).expect("signature"),
                    },
                }];
            } else {
                request.preserve_opaque_reasoning_state = true;
            }
            let (sink, _receiver) = mpsc::channel(4);
            let (_cancel, cancelled) = watch::channel(false);

            let error = provider
                .generate(request, Some("synthetic-secret"), sink, cancelled)
                .await
                .expect_err("unreplayable Gemini opaque state must fail closed");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert_eq!(error.message, GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR);
            assert!(!format!("{error:?}").contains(canary));
            assert_eq!(
                listener.accept().expect_err("must not connect").kind(),
                io::ErrorKind::WouldBlock
            );
        }
    }

    #[tokio::test]
    async fn exact_prior_part_topology_is_required_before_replaying_any_signature() {
        // Google requires the full original Content/Part topology and forbids
        // merging signed and unsigned Parts. A flattened assistant message
        // cannot prove exact replay even when the signature says part zero.
        for part_index in [0, 3] {
            let canary = format!("prior-gemini-signature-{part_index}");
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.set_nonblocking(true).expect("nonblocking");
            let base_url = format!("http://{}/v1beta", listener.local_addr().expect("address"));
            let provider = GeminiGenerateContentProvider::new(&base_url, Duration::from_secs(1))
                .expect("provider");
            let mut request = request();
            let source_message_id = request.messages[2].id.clone();
            let model_route_id = ModelRouteId::from("gemini-route");
            request.provider_provenance = Some(GenerationProviderProvenance {
                api_family: ApiFamily::GeminiGenerateContent,
                model_route_id: model_route_id.clone(),
                generation_preset_id: GenerationPresetId::from("current-preset"),
            });
            request.preserve_opaque_reasoning_state = true;
            request.opaque_reasoning_context = vec![OpaqueReasoningContext {
                source_message_id,
                api_family: ApiFamily::GeminiGenerateContent,
                model: request.model.clone(),
                model_route_id,
                generation_preset_id: GenerationPresetId::from("prior-preset"),
                state: OpaqueReasoningState::GeminiThoughtSignature {
                    part_index,
                    signature: OpaqueReasoningData::parse(&canary).expect("signature"),
                },
            }];
            let (sink, _receiver) = mpsc::channel(4);
            let (_cancel, cancelled) = watch::channel(false);

            let error = provider
                .generate(request, Some("synthetic-secret"), sink, cancelled)
                .await
                .expect_err("flattened topology must fail closed");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert!(error.message.contains("exact prior content topology"));
            assert!(!error.message.contains(&canary));
            assert!(!format!("{error:?}").contains(&canary));
            assert_eq!(
                listener.accept().expect_err("must not connect").kind(),
                io::ErrorKind::WouldBlock
            );
        }
    }

    #[tokio::test]
    async fn rejects_cross_route_thought_signature_before_network_access() {
        let canary = "cross-route-gemini-signature";
        let provider =
            GeminiGenerateContentProvider::new("http://127.0.0.1:9/v1beta", Duration::from_secs(1))
                .expect("provider");
        let mut request = request();
        let source_message_id = request.messages[2].id.clone();
        request.provider_provenance = Some(GenerationProviderProvenance {
            api_family: ApiFamily::GeminiGenerateContent,
            model_route_id: ModelRouteId::from("current-route"),
            generation_preset_id: GenerationPresetId::from("current-preset"),
        });
        request.preserve_opaque_reasoning_state = true;
        request.opaque_reasoning_context = vec![OpaqueReasoningContext {
            source_message_id,
            api_family: ApiFamily::GeminiGenerateContent,
            model: request.model.clone(),
            model_route_id: ModelRouteId::from("other-route"),
            generation_preset_id: GenerationPresetId::from("prior-preset"),
            state: OpaqueReasoningState::GeminiThoughtSignature {
                part_index: 0,
                signature: OpaqueReasoningData::parse(canary).expect("signature"),
            },
        }];
        let (sink, _receiver) = mpsc::channel(4);
        let (_cancel, cancelled) = watch::channel(false);

        let error = provider
            .generate(request, Some("synthetic-secret"), sink, cancelled)
            .await
            .expect_err("cross-route state must fail closed");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(!error.message.contains(canary));
    }

    #[tokio::test]
    async fn supports_unary_generate_content() {
        let body = br#"{
            "candidates":[{
                "content":{"parts":[{"text":"one"},{"text":"two","thought":true}]},
                "finishReason":"MAX_TOKENS",
                "index":0
            }],
            "usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}
        }"#;
        let (result, events, captured) =
            run(GeminiResponseMode::Unary, body, "application/json", 7).await;
        assert_eq!(
            result.expect("unary"),
            GenerationUsage {
                input_tokens: Some(3),
                output_tokens: Some(2),
                ..GenerationUsage::default()
            }
        );
        assert_eq!(
            events,
            vec![
                ProviderEvent::TextDelta("one".to_owned()),
                ProviderEvent::ReasoningDelta("two".to_owned()),
            ]
        );
        assert!(
            String::from_utf8(captured)
                .expect("request")
                .starts_with("POST /v1beta/models/gemini-2.5-flash:generateContent HTTP/1.1")
        );
    }

    #[tokio::test]
    async fn rejects_prompt_blocks_malformed_data_and_missing_finish() {
        for (body, expected) in [
            (
                "data: {\"promptFeedback\":{\"blockReason\":\"SAFETY\"}}\n\n",
                "Gemini blocked the prompt",
            ),
            (
                "data: {not-json}\n\n",
                "Gemini returned malformed response data",
            ),
            (
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n",
                "Gemini response ended without a finish reason",
            ),
            (
                "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n",
                "Gemini response did not contain a supported content part",
            ),
        ] {
            let (result, _, _) = run(
                GeminiResponseMode::Streaming,
                body.as_bytes(),
                "text/event-stream",
                2,
            )
            .await;
            assert_eq!(result.expect_err("must fail").message, expected);
        }
    }

    #[tokio::test]
    async fn rejects_policy_stops_unexpected_roles_and_invalid_part_shapes() {
        for (body, expected, recoverable) in [
            (
                concat!(
                    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n",
                    "data: {\"candidates\":[{\"finishReason\":\"SAFETY\"}]}\n\n",
                ),
                "Gemini stopped generation due to provider policy",
                false,
            ),
            (
                concat!(
                    "data: {\"candidates\":[{\"content\":{\"role\":\"user\",\"parts\":[{\"text\":\"wrong role\"}]}}]}\n\n",
                    "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n",
                ),
                "Gemini returned content with an unexpected role",
                true,
            ),
            (
                concat!(
                    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"mixed\",\"functionCall\":{\"name\":\"lookup\",\"args\":{}}}]}}]}\n\n",
                    "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n",
                ),
                "Gemini returned conflicting response part content",
                true,
            ),
            (
                concat!(
                    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"thoughtSignature\":\"YQ==\"}]}}]}\n\n",
                    "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n",
                ),
                "Gemini returned a response part without supported content",
                true,
            ),
        ] {
            let (result, _, _) = run(
                GeminiResponseMode::Streaming,
                body.as_bytes(),
                "text/event-stream",
                5,
            )
            .await;
            let error = result.expect_err("invalid response must fail");
            assert_eq!(error.message, expected);
            assert_eq!(error.recoverable, recoverable);
        }
    }

    #[tokio::test]
    async fn rejects_malformed_signatures_and_non_json_unary_responses() {
        let malformed_signature = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"answer\",\"thoughtSignature\":\"not-base64!\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n",
        );
        let (result, _, _) = run(
            GeminiResponseMode::Streaming,
            malformed_signature.as_bytes(),
            "text/event-stream",
            7,
        )
        .await;
        assert_eq!(
            result.expect_err("signature").message,
            "Gemini returned malformed opaque reasoning state"
        );

        let unary =
            br#"{"candidates":[{"content":{"parts":[{"text":"answer"}]},"finishReason":"STOP"}]}"#;
        let (result, events, _) = run(GeminiResponseMode::Unary, unary, "text/plain", 11).await;
        assert_eq!(
            result.expect_err("content type").message,
            "Gemini unary response was not JSON content"
        );
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn represents_function_calls_as_inert_protocol_events() {
        let body = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"lookup\",\"args\":{\"q\":\"seoul\"}}}]}}]}\n\n",
            "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n",
        );
        let (result, events, _) = run(
            GeminiResponseMode::Streaming,
            body.as_bytes(),
            "text/event-stream",
            body.len(),
        )
        .await;

        result.expect("Gemini function call must be a normal protocol state");
        assert!(matches!(
            &events[0],
            ProviderEvent::ToolCallStarted { id, name }
                if id.as_str() == "gemini-call-0" && name.as_str() == "lookup"
        ));
        assert!(matches!(
            &events[1],
            ProviderEvent::ToolCallArgumentsDelta { id, delta }
                if id.as_str() == "gemini-call-0"
                    && delta.as_str() == r#"{"q":"seoul"}"#
        ));
        assert!(matches!(
            &events[2],
            ProviderEvent::ToolCallCompleted { id } if id.as_str() == "gemini-call-0"
        ));
    }

    #[tokio::test]
    async fn rejects_unknown_parts_and_terminal_trailers() {
        for body in [
            concat!(
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"inlineData\":{\"mimeType\":\"text/plain\",\"data\":\"eA==\"}}]}}]}\n\n",
                "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n",
            ),
            concat!(
                "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n",
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"late\"}]}}]}\n\n",
            ),
        ] {
            let (result, events, _) = run(
                GeminiResponseMode::Streaming,
                body.as_bytes(),
                "text/event-stream",
                body.len(),
            )
            .await;
            assert_eq!(
                result.expect_err("must fail").code,
                CoreErrorCode::ProviderUnavailable
            );
            assert!(events.is_empty());
        }
    }

    #[tokio::test]
    async fn requires_key_and_sanitizes_header_errors() {
        let provider =
            GeminiGenerateContentProvider::new("http://127.0.0.1:9/v1beta", Duration::from_secs(1))
                .expect("provider");
        for credential in [None, Some("secret\r\ninjected: yes")] {
            let (sink, _events) = mpsc::channel(1);
            let (_cancel, cancelled) = watch::channel(false);
            let error = provider
                .generate(request(), credential, sink, cancelled)
                .await
                .expect_err("credential must fail");
            assert!(!error.message.contains("secret"));
            assert!(!error.message.contains("injected"));
        }
    }

    #[tokio::test]
    async fn maps_auth_rate_limit_and_does_not_follow_redirects() {
        for (status, expected_code) in [
            (
                "401 Unauthorized\r\nContent-Type: application/json",
                CoreErrorCode::ProviderAuthFailed,
            ),
            (
                "429 Too Many Requests\r\nContent-Type: application/json",
                CoreErrorCode::ProviderRateLimited,
            ),
            (
                "302 Found\r\nLocation: https://example.invalid/should-not-run\r\nContent-Type: application/json",
                CoreErrorCode::ProviderUnavailable,
            ),
        ] {
            let (base_url, _captured, handle) = deferred_server(status, b"", 1);
            let provider = GeminiGenerateContentProvider::new(&base_url, Duration::from_secs(2))
                .expect("provider");
            let (sink, _events) = mpsc::channel(1);
            let (_cancel, cancelled) = watch::channel(false);
            let error = provider
                .generate(request(), Some("synthetic-key"), sink, cancelled)
                .await
                .expect_err("HTTP status");
            assert_eq!(error.code, expected_code);
            assert!(!error.message.contains("example.invalid"));
            handle.join().expect("server");
        }
    }

    #[tokio::test]
    async fn cancellation_is_observed_before_connecting() {
        let provider =
            GeminiGenerateContentProvider::new("http://127.0.0.1:9/v1beta", Duration::from_secs(1))
                .expect("provider");
        let (sink, _events) = mpsc::channel(1);
        let (cancel, cancelled) = watch::channel(false);
        cancel.send(true).expect("cancel");
        let error = provider
            .generate(request(), Some("key"), sink, cancelled)
            .await
            .expect_err("cancelled");
        assert_eq!(error.code, CoreErrorCode::Cancelled);
    }

    #[tokio::test]
    async fn cancellation_interrupts_buffered_event_delivery() {
        let body = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"first\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"second\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n",
        );
        let (base_url, _captured, handle) = deferred_server(
            "200 OK\r\nContent-Type: text/event-stream",
            body.as_bytes(),
            body.len(),
        );
        let provider = GeminiGenerateContentProvider::new(&base_url, Duration::from_secs(2))
            .expect("provider");
        let (sink, mut events) = mpsc::channel(1);
        let (cancel, cancelled) = watch::channel(false);
        let generation = tokio::spawn(async move {
            provider
                .generate(request(), Some("synthetic-key"), sink, cancelled)
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
            .expect("task")
            .expect_err("cancelled generation");
        assert_eq!(error.code, CoreErrorCode::Cancelled);
        assert_eq!(
            events.try_recv().expect("buffered first event"),
            ProviderEvent::TextDelta("first".to_owned())
        );
        assert!(events.try_recv().is_err());
        handle.join().expect("server");
    }

    #[test]
    fn bounds_sse_events_even_with_separator_prefixes() {
        let mut pending = vec![b'x'; MAX_SSE_EVENT_BYTES];
        pending.push(b'\r');
        ensure_pending_size(&pending, false).expect("possible fragmented separator");
        assert!(ensure_pending_size(&pending, true).is_err());
        assert!(ensure_event_size(MAX_SSE_EVENT_BYTES).is_ok());
        assert!(ensure_event_size(MAX_SSE_EVENT_BYTES + 1).is_err());
    }

    #[test]
    fn rejects_invalid_generation_values_and_message_counts_before_networking() {
        for temperature in [f64::NAN, f64::INFINITY, -0.1] {
            let mut invalid = request();
            invalid.temperature = Some(temperature);
            let error = request_payload(invalid, true)
                .err()
                .expect("invalid temperature");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
        }

        let mut invalid = request();
        invalid.max_output_tokens = Some(0);
        assert_eq!(
            request_payload(invalid, true)
                .err()
                .expect("zero output limit")
                .code,
            CoreErrorCode::InvalidInput
        );

        let mut invalid = request();
        invalid.messages = vec![message(MessageRole::User, ""); MAX_GEMINI_REQUEST_MESSAGES + 1];
        assert_eq!(
            request_payload(invalid, true)
                .err()
                .expect("message count")
                .message,
            "Gemini request contains too many messages"
        );
    }

    #[tokio::test]
    async fn bounds_response_parts_before_persistence() {
        let parts = (0..=MAX_GEMINI_RESPONSE_PARTS)
            .map(|_| serde_json::json!({"text": ""}))
            .collect::<Vec<_>>();
        let body = serde_json::to_vec(&serde_json::json!({
            "candidates": [{
                "content": {"parts": parts},
                "finishReason": "STOP"
            }]
        }))
        .expect("response");
        let (result, _, _) = run(GeminiResponseMode::Unary, &body, "application/json", 4096).await;
        assert_eq!(
            result.expect_err("part count").message,
            "Gemini returned too many response parts"
        );
    }
}
