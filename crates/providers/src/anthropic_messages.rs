use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use async_trait::async_trait;
use futures_util::StreamExt;
use lorepia_domain::{
    AnthropicBlockText, AnthropicContentBlock, AnthropicContentBlockTopology, AnthropicToolInput,
    ApiFamily, AuthBinding, CacheTtl, CoreError, CoreErrorCode, CoreResult, GenerationRequest,
    GenerationUsage, MAX_ANTHROPIC_BLOCK_TEXT_CHARS, MAX_OPAQUE_REASONING_ITEM_BYTES,
    MAX_OPAQUE_REASONING_TOTAL_BYTES, MessageRole, OpaqueReasoningData, OpaqueReasoningState,
    ProviderCapabilities, ToolCallArgumentsDelta, ToolCallId, ToolName,
    validate_opaque_reasoning_states,
};
use reqwest::{
    Client, RequestBuilder, Response, StatusCode,
    header::{ACCEPT, CONTENT_TYPE},
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use url::{Host, Url};

use crate::{
    Provider, ProviderEvent, ProviderEventSender, merge_usage_summary,
    network_transport::{
        PreparedHttpTarget, ProviderHttpTarget, authorize_request, validate_credential_for_auth,
    },
    parameter_mapping::{PromptCacheDirective, PromptCacheWireDialect, ProviderRequestPlan},
    prompt_contract::{
        ProviderCacheBoundaryDisposition, ProviderCacheBoundaryStrategy,
        ProviderPromptAdapterContract, ProviderPromptPlacement,
    },
    request_plan::planned_json_payload,
};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_SSE_EVENT_BYTES: usize = 1024 * 1024;
const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
const MAX_CONTENT_BLOCKS: usize = 128;
const MAX_PROMPT_MESSAGES: usize = 128;
const MAX_PROMPT_BYTES: usize = 512 * 1024;
const MAX_PROMPT_CHARS: usize = 128 * 1024;
const MAX_MODEL_ID_BYTES: usize = 1024;
const MAX_MODEL_ID_CHARS: usize = 256;
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

/// Adapter for Anthropic's native Messages streaming API.
#[derive(Clone)]
pub struct AnthropicMessagesProvider {
    endpoint: Url,
    target: ProviderHttpTarget,
    auth: AuthBinding,
    request_plan: Option<ProviderRequestPlan>,
}

impl AnthropicMessagesProvider {
    /// Creates an adapter for an Anthropic-compatible origin.
    ///
    /// `base_url` must be an HTTPS origin (or loopback HTTP for local testing)
    /// with no path, query, fragment, or embedded credentials. The adapter
    /// always sends generation requests to `/v1/messages`.
    pub fn new(base_url: &str, timeout: Duration) -> CoreResult<Self> {
        if timeout.is_zero() {
            return Err(CoreError::invalid(
                "provider timeout must be greater than zero",
            ));
        }
        let endpoint = messages_endpoint(base_url)?;
        let target = ProviderHttpTarget::inferred(endpoint.as_str(), timeout)?;
        Ok(Self {
            endpoint,
            target,
            auth: AuthBinding::HeaderApiKey {
                header_name: lorepia_domain::HeaderName::parse("x-api-key")
                    .map_err(CoreError::internal)?,
            },
            request_plan: None,
        })
    }

    pub(crate) fn new_with_manifest_target(target: ProviderHttpTarget, auth: AuthBinding) -> Self {
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

    #[cfg(test)]
    fn generation_request(
        &self,
        client: &Client,
        request: GenerationRequest,
        credential: Option<&str>,
    ) -> CoreResult<RequestBuilder> {
        let has_resolved_prompt = request.resolved_prompt_plan.is_some();
        let cache_dialect = self
            .request_plan
            .as_ref()
            .map_or(PromptCacheWireDialect::Unsupported, |plan| {
                plan.cache_dialect()
            });
        let payload = request_payload_with_cache(request, cache_dialect)?;
        let request_plan = effective_request_plan(self.request_plan.as_ref(), has_resolved_prompt);
        let mut payload = planned_json_payload(
            &payload,
            ApiFamily::AnthropicMessages,
            request_plan.as_ref(),
        )?;
        omit_incompatible_base_temperature(&mut payload, request_plan.as_ref())?;
        validate_planned_max_tokens(&payload)?;
        authorize_request(
            client
                .post(self.endpoint.clone())
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header(ACCEPT, "text/event-stream")
                .json(&payload),
            &self.auth,
            credential,
        )
    }

    fn planned_generation_payload(
        &self,
        request: GenerationRequest,
    ) -> CoreResult<serde_json::Value> {
        let has_resolved_prompt = request.resolved_prompt_plan.is_some();
        let cache_dialect = self
            .request_plan
            .as_ref()
            .map_or(PromptCacheWireDialect::Unsupported, |plan| {
                plan.cache_dialect()
            });
        let payload = request_payload_with_cache(request, cache_dialect)?;
        let request_plan = effective_request_plan(self.request_plan.as_ref(), has_resolved_prompt);
        let mut payload = planned_json_payload(
            &payload,
            ApiFamily::AnthropicMessages,
            request_plan.as_ref(),
        )?;
        omit_incompatible_base_temperature(&mut payload, request_plan.as_ref())?;
        validate_planned_max_tokens(&payload)?;
        Ok(payload)
    }

    fn authorize_generation_payload(
        &self,
        client: &Client,
        payload: &serde_json::Value,
        credential: Option<&str>,
    ) -> CoreResult<RequestBuilder> {
        authorize_request(
            client
                .post(self.endpoint.clone())
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header(ACCEPT, "text/event-stream")
                .json(payload),
            &self.auth,
            credential,
        )
    }

    async fn send_generation_request(
        &self,
        request: GenerationRequest,
        credential: Option<&str>,
        cancelled: &mut watch::Receiver<bool>,
    ) -> CoreResult<(PreparedHttpTarget, Response, bool)> {
        ensure_not_cancelled(cancelled)?;
        let payload = self.planned_generation_payload(request)?;
        validate_credential_for_auth(&self.auth, credential)?;
        let prepared = self.target.prepare().await?;
        ensure_not_cancelled(cancelled)?;
        let request = self
            .authorize_generation_payload(prepared.client(), &payload, credential)?
            .send();
        tokio::pin!(request);

        let mut cancellation_open = true;
        let response = loop {
            tokio::select! {
                change = cancelled.changed(), if cancellation_open => {
                    if change.is_ok() {
                        ensure_not_cancelled(cancelled)?;
                    } else {
                        cancellation_open = false;
                    }
                }
                result = &mut request => {
                    break result.map_err(network_error)?;
                }
            }
        };
        Ok((prepared, response, cancellation_open))
    }
}

#[async_trait]
impl Provider for AnthropicMessagesProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: true,
            max_context_tokens: None,
        }
    }

    fn snapshot_request(&self, request: &GenerationRequest) -> CoreResult<serde_json::Value> {
        crate::validate_snapshot_request(request)?;
        self.planned_generation_payload(request.clone())
    }

    async fn generate(
        &self,
        request: GenerationRequest,
        credential: Option<&str>,
        sink: ProviderEventSender,
        mut cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        let preserve_opaque_reasoning_state = request.preserve_opaque_reasoning_state;
        let (prepared, response, mut cancellation_open) = self
            .send_generation_request(request, credential, &mut cancelled)
            .await?;

        prepared.validate_response_peer(&response)?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        ensure_event_stream_content_type(response.headers())?;

        let mut bytes = response.bytes_stream();
        let mut pending = Vec::<u8>::new();
        let mut total_stream_bytes = 0_usize;
        let mut usage = GenerationUsage::default();
        let mut state = StreamState::default();

        loop {
            tokio::select! {
                change = cancelled.changed(), if cancellation_open => {
                    if change.is_ok() {
                        ensure_not_cancelled(&cancelled)?;
                    } else {
                        cancellation_open = false;
                    }
                }
                chunk = bytes.next() => {
                    let Some(chunk) = chunk else { break };
                    let chunk = chunk.map_err(network_error)?;
                    total_stream_bytes = total_stream_bytes
                        .checked_add(chunk.len())
                        .filter(|total| *total <= MAX_STREAM_BYTES)
                        .ok_or_else(|| streaming_error(
                            "provider stream exceeded 64 MiB",
                        ))?;
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
                                &mut state,
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
                &mut state,
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
        Err(state.incomplete_error())
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

fn messages_endpoint(base_url: &str) -> CoreResult<Url> {
    let mut endpoint =
        Url::parse(base_url).map_err(|_| CoreError::invalid("invalid provider base URL"))?;
    if endpoint.cannot_be_a_base() || endpoint.host().is_none() {
        return Err(CoreError::invalid(
            "provider base URL must be an absolute network origin",
        ));
    }
    if endpoint.username() != "" || endpoint.password().is_some() {
        return Err(CoreError::invalid(
            "provider URL must not contain embedded credentials",
        ));
    }
    if endpoint.query().is_some() || endpoint.fragment().is_some() {
        return Err(CoreError::invalid(
            "provider base URL must not contain a query or fragment",
        ));
    }
    if !matches!(endpoint.path(), "" | "/") {
        return Err(CoreError::invalid(
            "provider base URL must not contain a path",
        ));
    }
    match endpoint.scheme() {
        "https" => {}
        "http" if is_loopback_host(&endpoint) => {}
        "http" => {
            return Err(CoreError::invalid(
                "unencrypted HTTP is allowed only for loopback endpoints",
            ));
        }
        _ => {
            return Err(CoreError::invalid(
                "provider endpoint must use HTTPS or loopback HTTP",
            ));
        }
    }
    endpoint.set_path("/v1/messages");
    Ok(endpoint)
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn effective_request_plan(
    request_plan: Option<&ProviderRequestPlan>,
    has_resolved_prompt: bool,
) -> Option<ProviderRequestPlan> {
    request_plan.cloned().map(|mut plan| {
        if has_resolved_prompt
            && matches!(
                &plan.prompt_cache,
                PromptCacheDirective::AnthropicExplicitBreakpoints { .. }
            )
        {
            plan.prompt_cache = PromptCacheDirective::None;
        }
        plan
    })
}

#[cfg(test)]
fn request_payload(request: GenerationRequest) -> CoreResult<RequestPayload> {
    request_payload_with_cache(request, PromptCacheWireDialect::Unsupported)
}

// Keep resolved and legacy Anthropic topology replay in one audited function:
// both branches enforce the same one-to-one opaque-state consumption rule.
#[allow(clippy::too_many_lines)]
fn request_payload_with_cache(
    request: GenerationRequest,
    cache_dialect: PromptCacheWireDialect,
) -> CoreResult<RequestPayload> {
    validate_request(&request)?;

    if let Some(plan) = &request.resolved_prompt_plan {
        let compiled = prompt_contract()
            .compile_resolved_plan_for_execution(plan, cache_dialect)
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        compiled
            .verify_execution_hash(request.provider_execution_plan_hash.as_deref())
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        let mut cache_boundaries = BTreeMap::new();
        for boundary in compiled.cache_boundaries() {
            if matches!(
                boundary.disposition,
                ProviderCacheBoundaryDisposition::Mapped {
                    strategy: ProviderCacheBoundaryStrategy::AnthropicInlineBreakpoint,
                }
            ) {
                let sequence = boundary.after_message_sequence.ok_or_else(|| {
                    CoreError::invalid("resolved Anthropic cache boundary has no message sequence")
                })?;
                if cache_boundaries.insert(sequence, boundary.ttl).is_some() {
                    return Err(CoreError::invalid(
                        "resolved Anthropic prompt repeats a cache boundary sequence",
                    ));
                }
            }
        }

        let mut system_parts = Vec::new();
        let mut messages = Vec::new();
        let mut used_context_messages = BTreeSet::new();
        for message in compiled.messages() {
            let cache_control = cache_boundaries
                .remove(&message.sequence())
                .map(anthropic_cache_control_for_ttl);
            if message.placement() == ProviderPromptPlacement::SystemInstruction {
                system_parts.push(RequestTextBlock {
                    kind: "text",
                    text: message.content().to_owned(),
                    cache_control,
                });
                continue;
            }

            let content = if request.preserve_opaque_reasoning_state
                && message.effective_role() == lorepia_domain::ProviderMessageRole::Assistant
            {
                let mut matching = request.opaque_reasoning_context.iter().filter(|context| {
                    message
                        .source_message_ids()
                        .contains(&context.source_message_id)
                });
                match (matching.next(), matching.next()) {
                    (Some(context), None) => {
                        if cache_control.is_some() {
                            return Err(CoreError::invalid(
                                "Anthropic cache boundary cannot rewrite opaque assistant topology",
                            ));
                        }
                        let OpaqueReasoningState::AnthropicMessages { content_blocks } =
                            &context.state
                        else {
                            return Err(CoreError::invalid(
                                "opaque reasoning state format does not match Anthropic",
                            ));
                        };
                        if content_blocks.flattened_text() != message.content() {
                            return Err(CoreError::invalid(
                                "stored Anthropic content topology does not match its source message",
                            ));
                        }
                        if !used_context_messages.insert(context.source_message_id.0.clone()) {
                            return Err(CoreError::invalid(
                                "opaque reasoning state was reused by multiple resolved prompt messages",
                            ));
                        }
                        RequestMessageContent::Blocks(content_blocks.blocks().to_vec())
                    }
                    (Some(_), Some(_)) => {
                        return Err(CoreError::invalid(
                            "stored Anthropic content topology is duplicated",
                        ));
                    }
                    (None, _) => RequestMessageContent::PromptBlocks(vec![RequestTextBlock {
                        kind: "text",
                        text: message.content().to_owned(),
                        cache_control,
                    }]),
                }
            } else {
                RequestMessageContent::PromptBlocks(vec![RequestTextBlock {
                    kind: "text",
                    text: message.content().to_owned(),
                    cache_control,
                }])
            };
            messages.push(RequestMessage {
                role: message.wire_role().as_str(),
                content,
            });
        }
        if !cache_boundaries.is_empty() {
            return Err(CoreError::invalid(
                "resolved Anthropic cache boundary targets a missing message",
            ));
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
        if messages.is_empty() {
            return Err(CoreError::invalid(
                "Anthropic Messages requests require at least one user or assistant message",
            ));
        }
        return Ok(RequestPayload {
            model: request.model,
            messages,
            system: (!system_parts.is_empty()).then_some(SystemPrompt::Blocks(system_parts)),
            max_tokens: request.max_output_tokens,
            temperature: request.temperature,
            stream: true,
        });
    }

    let mut system_parts = Vec::new();
    let mut messages = Vec::new();
    let mut used_context_messages = BTreeSet::new();
    for message in request.messages {
        match message.role {
            MessageRole::System => {
                debug_assert_eq!(
                    prompt_contract()
                        .map_message_role(MessageRole::System)
                        .wire_role
                        .as_str(),
                    "system"
                );
                system_parts.push(message.content);
            }
            MessageRole::User => messages.push(RequestMessage {
                role: role_name(MessageRole::User),
                content: RequestMessageContent::Text(message.content),
            }),
            MessageRole::Assistant => {
                let content = if request.preserve_opaque_reasoning_state {
                    let mut matching = request
                        .opaque_reasoning_context
                        .iter()
                        .filter(|context| context.source_message_id == message.id);
                    match (matching.next(), matching.next()) {
                        (Some(context), None) => {
                            let OpaqueReasoningState::AnthropicMessages { content_blocks } =
                                &context.state
                            else {
                                return Err(CoreError::invalid(
                                    "opaque reasoning state format does not match Anthropic",
                                ));
                            };
                            if content_blocks.flattened_text() != message.content {
                                return Err(CoreError::invalid(
                                    "stored Anthropic content topology does not match its source message",
                                ));
                            }
                            if !used_context_messages.insert(message.id.0.clone()) {
                                return Err(CoreError::invalid(
                                    "opaque reasoning state was reused by multiple prompt messages",
                                ));
                            }
                            RequestMessageContent::Blocks(content_blocks.blocks().to_vec())
                        }
                        (Some(_), Some(_)) => {
                            return Err(CoreError::invalid(
                                "stored Anthropic content topology is duplicated",
                            ));
                        }
                        (None, _) => RequestMessageContent::Text(message.content),
                    }
                } else {
                    RequestMessageContent::Text(message.content)
                };
                messages.push(RequestMessage {
                    role: role_name(MessageRole::Assistant),
                    content,
                });
            }
        }
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
    if messages.is_empty() {
        return Err(CoreError::invalid(
            "Anthropic Messages requests require at least one user or assistant message",
        ));
    }

    Ok(RequestPayload {
        model: request.model,
        messages,
        system: (!system_parts.is_empty()).then(|| SystemPrompt::Text(system_parts.join("\n\n"))),
        max_tokens: request.max_output_tokens,
        temperature: request.temperature,
        stream: true,
    })
}

pub(crate) const fn prompt_contract() -> ProviderPromptAdapterContract {
    ProviderPromptAdapterContract::for_family(ApiFamily::AnthropicMessages)
}

#[cfg(test)]
pub(crate) fn resolved_prompt_payload_for_contract_test(
    request: GenerationRequest,
    cache_dialect: PromptCacheWireDialect,
) -> CoreResult<serde_json::Value> {
    let payload = request_payload_with_cache(request, cache_dialect)?;
    serde_json::to_value(payload)
        .map_err(|_| CoreError::internal("failed to serialize Anthropic test payload"))
}

fn role_name(role: MessageRole) -> &'static str {
    prompt_contract().map_message_role(role).wire_role.as_str()
}

const fn anthropic_cache_control_for_ttl(ttl: CacheTtl) -> RequestCacheControl {
    RequestCacheControl {
        kind: "ephemeral",
        ttl: match ttl {
            CacheTtl::ProviderDefault | CacheTtl::Short => None,
            CacheTtl::Long => Some("1h"),
        },
    }
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
            "Anthropic model is not a bounded identifier",
        ));
    }
    let prompt_message_count = request
        .resolved_prompt_plan
        .as_ref()
        .map_or(request.messages.len(), |plan| plan.effective_messages.len());
    if prompt_message_count > MAX_PROMPT_MESSAGES {
        return Err(CoreError::invalid(
            "Anthropic prompt exceeded the message-count safety limit",
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
                    "Anthropic prompt contains a message from another conversation",
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
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(CoreError::invalid(
            "Anthropic temperature must be between 0 and 1",
        ));
    }
    if request.max_output_tokens == Some(0) {
        return Err(CoreError::invalid(
            "Anthropic max output tokens must be greater than zero",
        ));
    }
    if !request.preserve_opaque_reasoning_state {
        return Ok(());
    }
    validate_opaque_context(request)
}

fn validate_opaque_context(request: &GenerationRequest) -> CoreResult<()> {
    let provenance = request
        .provider_provenance
        .as_ref()
        .ok_or_else(|| CoreError::invalid("opaque reasoning state requires provider provenance"))?;
    if provenance.api_family != ApiFamily::AnthropicMessages {
        return Err(CoreError::invalid(
            "opaque reasoning state API family does not match Anthropic",
        ));
    }
    let states = request
        .opaque_reasoning_context
        .iter()
        .map(|context| {
            if context.api_family != ApiFamily::AnthropicMessages
                || context.model != request.model
                || context.model_route_id != provenance.model_route_id
            {
                return Err(CoreError::invalid(
                    "opaque reasoning state provenance does not match the current provider route",
                ));
            }
            if !matches!(
                context.state,
                OpaqueReasoningState::AnthropicMessages { .. }
            ) {
                return Err(CoreError::invalid(
                    "opaque reasoning state format does not match Anthropic",
                ));
            }
            let OpaqueReasoningState::AnthropicMessages { content_blocks } = &context.state else {
                unreachable!("Anthropic state format was checked above");
            };
            if content_blocks
                .blocks()
                .iter()
                .any(|block| matches!(block, AnthropicContentBlock::ToolUse { .. }))
            {
                return Err(CoreError::invalid(
                    "Anthropic tool-use topology cannot be replayed without exact tool results",
                ));
            }
            Ok(context.state.clone())
        })
        .collect::<CoreResult<Vec<_>>>()?;
    validate_opaque_reasoning_states(&states).map_err(CoreError::invalid)
}

fn prompt_too_large_error() -> CoreError {
    CoreError::invalid("Anthropic prompt exceeded the input safety limit")
}

fn validate_planned_max_tokens(payload: &serde_json::Value) -> CoreResult<()> {
    let max_tokens = payload
        .get("max_tokens")
        .and_then(serde_json::Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            CoreError::invalid("Anthropic max_tokens must be an explicit positive integer")
        })?;
    if payload
        .pointer("/thinking/budget_tokens")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|budget| budget >= max_tokens)
    {
        return Err(CoreError::invalid(
            "Anthropic thinking budget must be less than max_tokens",
        ));
    }
    Ok(())
}

fn omit_incompatible_base_temperature(
    payload: &mut serde_json::Value,
    request_plan: Option<&ProviderRequestPlan>,
) -> CoreResult<()> {
    let thinking_active = payload
        .pointer("/thinking/type")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|kind| matches!(kind, "adaptive" | "enabled"));
    if thinking_active
        && request_plan.is_some_and(|plan| {
            plan.body_patches()
                .iter()
                .any(|patch| patch.path == "temperature")
        })
    {
        return Err(CoreError::invalid(
            "Anthropic thinking cannot be combined with an explicit temperature",
        ));
    }
    if thinking_active && let Some(payload) = payload.as_object_mut() {
        payload.remove("temperature");
    }
    Ok(())
}

#[derive(Serialize)]
struct RequestPayload {
    model: String,
    messages: Vec<RequestMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<SystemPrompt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    stream: bool,
}

#[derive(Serialize)]
#[serde(untagged)]
enum SystemPrompt {
    Text(String),
    Blocks(Vec<RequestTextBlock>),
}

#[derive(Serialize)]
struct RequestMessage {
    role: &'static str,
    content: RequestMessageContent,
}

#[derive(Serialize)]
#[serde(untagged)]
enum RequestMessageContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
    PromptBlocks(Vec<RequestTextBlock>),
}

#[derive(Serialize)]
struct RequestTextBlock {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<RequestCacheControl>,
}

#[derive(Serialize)]
struct RequestCacheControl {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl: Option<&'static str>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum StreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: MessageStart },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: ContentDelta },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: ContentBlock,
    },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: serde_json::Map<String, serde_json::Value>,
        #[serde(default)]
        usage: Option<StreamUsage>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "error")]
    Error { error: StreamError },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct MessageStart {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    usage: Option<StreamUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum ContentDelta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
    #[serde(rename = "signature_delta")]
    Signature { signature: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct StreamUsage {
    input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    output_tokens_details: Option<OutputTokensDetails>,
    cache_creation: Option<CacheCreationDetails>,
}

#[derive(Deserialize)]
struct OutputTokensDetails {
    thinking_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct CacheCreationDetails {
    ephemeral_5m_input_tokens: Option<u64>,
    ephemeral_1h_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct StreamError {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Default)]
struct StreamState {
    started: bool,
    message_delta_seen: bool,
    active_blocks: BTreeMap<u32, ActiveContentBlock>,
    completed_blocks: BTreeMap<u32, AnthropicContentBlock>,
    seen_blocks: BTreeSet<u32>,
    tool_call_ids: BTreeSet<ToolCallId>,
}

enum ActiveContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: String,
        signature_seen: bool,
    },
    RedactedThinking {
        data: OpaqueReasoningData,
    },
    ToolUse {
        id: ToolCallId,
        name: ToolName,
        input_json: String,
    },
}

impl StreamState {
    fn incomplete_error(&self) -> CoreError {
        if self.started {
            streaming_error("provider stream ended before message_stop")
        } else {
            streaming_error("provider returned an empty streaming response")
        }
    }

    fn ensure_no_active_blocks(&self) -> CoreResult<()> {
        if self.active_blocks.is_empty() {
            Ok(())
        } else {
            Err(streaming_error(
                "provider ended before completing every content block",
            ))
        }
    }
}

fn ensure_reasoning_prefix(state: &StreamState) -> CoreResult<()> {
    if state
        .completed_blocks
        .values()
        .any(|block| matches!(block, AnthropicContentBlock::Text { .. }))
    {
        Err(streaming_error(
            "provider returned Anthropic reasoning after a visible content block",
        ))
    } else {
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum EventAction {
    Continue,
    Done,
}

async fn process_event(
    event: &[u8],
    sink: &ProviderEventSender,
    usage: &mut GenerationUsage,
    state: &mut StreamState,
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
    if data.trim().is_empty() {
        return Ok(EventAction::Continue);
    }

    let event: StreamEvent = serde_json::from_str(&data)
        .map_err(|_| streaming_error("provider returned malformed streaming data"))?;
    match event {
        StreamEvent::MessageStart { message } => {
            if state.started {
                return Err(streaming_error(
                    "provider returned a duplicate message_start event",
                ));
            }
            if message.role.as_deref() != Some("assistant")
                || message
                    .content
                    .as_ref()
                    .is_none_or(|content| !content.is_empty())
            {
                return Err(streaming_error(
                    "provider returned an invalid Anthropic message_start",
                ));
            }
            state.started = true;
            update_usage(message.usage.as_ref(), usage)?;
        }
        StreamEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            ensure_started(state)?;
            start_content_block(
                index,
                content_block,
                state,
                sink,
                cancelled,
                cancellation_open,
            )
            .await?;
        }
        StreamEvent::ContentBlockDelta { index, delta } => {
            ensure_started(state)?;
            process_content_delta(index, delta, state, sink, cancelled, cancellation_open).await?;
        }
        StreamEvent::ContentBlockStop { index } => {
            ensure_started(state)?;
            finish_content_block(index, state, sink, cancelled, cancellation_open).await?;
        }
        StreamEvent::MessageDelta {
            delta,
            usage: stream_usage,
        } => {
            let _ = delta;
            ensure_started(state)?;
            state.ensure_no_active_blocks()?;
            state.message_delta_seen = true;
            update_usage(stream_usage.as_ref(), usage)?;
        }
        StreamEvent::MessageStop => {
            ensure_started(state)?;
            state.ensure_no_active_blocks()?;
            if !state.message_delta_seen {
                return Err(streaming_error(
                    "provider returned message_stop before message_delta",
                ));
            }
            emit_anthropic_topology(
                state,
                preserve_opaque_reasoning_state,
                sink,
                cancelled,
                cancellation_open,
            )
            .await?;
            return Ok(EventAction::Done);
        }
        StreamEvent::Error { error } => return Err(stream_error(&error.kind)),
        StreamEvent::Other => {}
    }
    Ok(EventAction::Continue)
}

async fn start_content_block(
    index: u32,
    block: ContentBlock,
    state: &mut StreamState,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<()> {
    register_content_block_index(index, state)?;

    let active = match block {
        ContentBlock::Text { text } => {
            if !text.is_empty() {
                return Err(streaming_error(
                    "provider returned a non-empty text block start",
                ));
            }
            validate_block_text(&text)?;
            ActiveContentBlock::Text { text }
        }
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
            ensure_reasoning_prefix(state)?;
            if !thinking.is_empty() {
                return Err(streaming_error(
                    "provider returned a non-empty thinking block start",
                ));
            }
            validate_block_text(&thinking)?;
            if signature.as_deref() != Some("") {
                return Err(streaming_error(
                    "provider returned an invalid thinking block start",
                ));
            }
            ActiveContentBlock::Thinking {
                thinking,
                signature: String::new(),
                signature_seen: false,
            }
        }
        ContentBlock::ToolUse { id, name, input } => {
            if input.as_object().is_none_or(|input| !input.is_empty()) {
                return Err(streaming_error(
                    "Anthropic tool block must begin with an empty input object",
                ));
            }
            let id = ToolCallId::parse(id)
                .map_err(|_| streaming_error("provider returned an invalid tool-call id"))?;
            if !state.tool_call_ids.insert(id.clone()) {
                return Err(streaming_error("provider reused an Anthropic tool-call id"));
            }
            let name = ToolName::parse(name)
                .map_err(|_| streaming_error("provider returned an invalid tool name"))?;
            send_provider_event(
                sink,
                ProviderEvent::ToolCallStarted {
                    id: id.clone(),
                    name: name.clone(),
                },
                cancelled,
                cancellation_open,
            )
            .await?;
            ActiveContentBlock::ToolUse {
                id,
                name,
                input_json: String::new(),
            }
        }
        ContentBlock::RedactedThinking { data } => {
            ensure_reasoning_prefix(state)?;
            ActiveContentBlock::RedactedThinking {
                data: OpaqueReasoningData::parse(data).map_err(|_| {
                    streaming_error("provider returned invalid redacted thinking data")
                })?,
            }
        }
        ContentBlock::Other => {
            return Err(streaming_error(
                "provider returned an unsupported content block",
            ));
        }
    };
    state.active_blocks.insert(index, active);
    Ok(())
}

fn register_content_block_index(index: u32, state: &mut StreamState) -> CoreResult<()> {
    if state.message_delta_seen {
        return Err(streaming_error(
            "provider returned a content block after message_delta",
        ));
    }
    if !state.active_blocks.is_empty() {
        return Err(streaming_error(
            "provider started overlapping Anthropic content blocks",
        ));
    }
    if state.seen_blocks.len() >= MAX_CONTENT_BLOCKS {
        return Err(streaming_error(
            "provider exceeded the 128 content-block limit",
        ));
    }
    let expected_index = u32::try_from(state.seen_blocks.len())
        .map_err(|_| streaming_error("provider content-block index overflowed"))?;
    if index != expected_index || !state.seen_blocks.insert(index) {
        return Err(streaming_error(
            "provider returned a non-contiguous Anthropic content-block index",
        ));
    }
    Ok(())
}

async fn process_content_delta(
    index: u32,
    delta: ContentDelta,
    state: &mut StreamState,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<()> {
    let active = state.active_blocks.get_mut(&index).ok_or_else(|| {
        streaming_error("provider returned a delta outside an active content block")
    })?;
    let event = match (active, delta) {
        (ActiveContentBlock::Text { text: accumulated }, ContentDelta::Text { text }) => {
            append_block_text(accumulated, &text)?;
            (!text.is_empty()).then_some(ProviderEvent::TextDelta(text))
        }
        (
            ActiveContentBlock::Thinking {
                thinking: accumulated,
                signature_seen,
                ..
            },
            ContentDelta::Thinking { thinking },
        ) => {
            if *signature_seen {
                return Err(streaming_error(
                    "provider returned thinking data after its signature",
                ));
            }
            append_block_text(accumulated, &thinking)?;
            (!thinking.is_empty()).then_some(ProviderEvent::ReasoningDelta(thinking))
        }
        (
            ActiveContentBlock::Thinking {
                signature: accumulated,
                signature_seen,
                ..
            },
            ContentDelta::Signature { signature },
        ) => {
            if *signature_seen || signature.is_empty() {
                return Err(streaming_error(
                    "provider returned an invalid thinking signature sequence",
                ));
            }
            append_private_fragment(accumulated, &signature)?;
            *signature_seen = true;
            None
        }
        (
            ActiveContentBlock::ToolUse {
                id,
                input_json: accumulated,
                ..
            },
            ContentDelta::InputJson { partial_json },
        ) => {
            append_tool_input_fragment(accumulated, &partial_json)?;
            if partial_json.is_empty() {
                None
            } else {
                let delta = ToolCallArgumentsDelta::parse(partial_json).map_err(|_| {
                    streaming_error("provider returned invalid tool-call argument data")
                })?;
                Some(ProviderEvent::ToolCallArgumentsDelta {
                    id: id.clone(),
                    delta,
                })
            }
        }
        (_, ContentDelta::Other) => {
            return Err(streaming_error(
                "provider returned an unsupported content delta",
            ));
        }
        _ => {
            return Err(streaming_error(
                "provider content delta did not match its active block",
            ));
        }
    };
    if let Some(event) = event {
        send_provider_event(sink, event, cancelled, cancellation_open).await?;
    }
    Ok(())
}

async fn finish_content_block(
    index: u32,
    state: &mut StreamState,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<()> {
    let active = state
        .active_blocks
        .remove(&index)
        .ok_or_else(|| streaming_error("provider stopped a content block that was not active"))?;
    let (block, completed_tool_id) = match active {
        ActiveContentBlock::Text { text } => (
            AnthropicContentBlock::Text {
                text: AnthropicBlockText::parse(text)
                    .map_err(|_| streaming_error("provider returned invalid text block"))?,
            },
            None,
        ),
        ActiveContentBlock::Thinking {
            thinking,
            signature,
            signature_seen: _,
        } => (
            AnthropicContentBlock::Thinking {
                thinking: AnthropicBlockText::parse(thinking)
                    .map_err(|_| streaming_error("provider returned invalid thinking block"))?,
                signature: OpaqueReasoningData::parse(signature)
                    .map_err(|_| streaming_error("provider returned invalid thinking signature"))?,
            },
            None,
        ),
        ActiveContentBlock::RedactedThinking { data } => {
            (AnthropicContentBlock::RedactedThinking { data }, None)
        }
        ActiveContentBlock::ToolUse {
            id,
            name,
            input_json,
        } => {
            let input_json = if input_json.is_empty() {
                "{}"
            } else {
                input_json.as_str()
            };
            let input = serde_json::from_str(input_json)
                .map_err(|_| streaming_error("provider returned invalid tool-call arguments"))?;
            let input = AnthropicToolInput::from_value(&input)
                .map_err(|_| streaming_error("provider returned invalid tool-call arguments"))?;
            let completed_id = id.clone();
            (
                AnthropicContentBlock::ToolUse { id, name, input },
                Some(completed_id),
            )
        }
    };
    if state.completed_blocks.insert(index, block).is_some() {
        return Err(streaming_error(
            "provider reused an Anthropic content-block index",
        ));
    }
    if let Some(id) = completed_tool_id {
        send_provider_event(
            sink,
            ProviderEvent::ToolCallCompleted { id },
            cancelled,
            cancellation_open,
        )
        .await?;
    }
    Ok(())
}

async fn emit_anthropic_topology(
    state: &StreamState,
    preserve_opaque_reasoning_state: bool,
    sink: &ProviderEventSender,
    cancelled: &mut watch::Receiver<bool>,
    cancellation_open: &mut bool,
) -> CoreResult<()> {
    if state.completed_blocks.len() != state.seen_blocks.len() {
        return Err(streaming_error(
            "provider ended before completing every content block",
        ));
    }
    let has_reasoning = state.completed_blocks.values().any(|block| {
        matches!(
            block,
            AnthropicContentBlock::Thinking { .. } | AnthropicContentBlock::RedactedThinking { .. }
        )
    });
    if !has_reasoning || !preserve_opaque_reasoning_state {
        return Ok(());
    }
    if state
        .completed_blocks
        .values()
        .any(|block| matches!(block, AnthropicContentBlock::ToolUse { .. }))
    {
        return Err(streaming_error(
            "provider returned an Anthropic tool-use topology without exact tool results",
        ));
    }
    let topology =
        AnthropicContentBlockTopology::new(state.completed_blocks.values().cloned().collect())
            .map_err(|_| {
                streaming_error("provider returned an invalid Anthropic content topology")
            })?;
    send_provider_event(
        sink,
        ProviderEvent::OpaqueReasoningState(OpaqueReasoningState::AnthropicMessages {
            content_blocks: topology,
        }),
        cancelled,
        cancellation_open,
    )
    .await
}

fn validate_block_text(value: &str) -> CoreResult<()> {
    if value.len() > MAX_OPAQUE_REASONING_TOTAL_BYTES
        || value.chars().count() > MAX_ANTHROPIC_BLOCK_TEXT_CHARS
    {
        Err(streaming_error(
            "provider Anthropic text block exceeded its safety limit",
        ))
    } else {
        Ok(())
    }
}

fn append_block_text(target: &mut String, fragment: &str) -> CoreResult<()> {
    let bytes = target
        .len()
        .checked_add(fragment.len())
        .ok_or_else(|| streaming_error("provider Anthropic text block size overflowed"))?;
    let chars = target
        .chars()
        .count()
        .checked_add(fragment.chars().count())
        .ok_or_else(|| streaming_error("provider Anthropic text block size overflowed"))?;
    if bytes > MAX_OPAQUE_REASONING_TOTAL_BYTES || chars > MAX_ANTHROPIC_BLOCK_TEXT_CHARS {
        return Err(streaming_error(
            "provider Anthropic text block exceeded its safety limit",
        ));
    }
    target.push_str(fragment);
    Ok(())
}

fn append_private_fragment(target: &mut String, fragment: &str) -> CoreResult<()> {
    if target
        .len()
        .checked_add(fragment.len())
        .is_none_or(|length| length > MAX_OPAQUE_REASONING_ITEM_BYTES)
    {
        return Err(streaming_error(
            "provider Anthropic opaque block exceeded its safety limit",
        ));
    }
    target.push_str(fragment);
    Ok(())
}

fn append_tool_input_fragment(target: &mut String, fragment: &str) -> CoreResult<()> {
    if target
        .len()
        .checked_add(fragment.len())
        .is_none_or(|length| length > MAX_OPAQUE_REASONING_ITEM_BYTES)
    {
        return Err(streaming_error(
            "provider Anthropic tool input exceeded its safety limit",
        ));
    }
    target.push_str(fragment);
    Ok(())
}

fn ensure_started(state: &StreamState) -> CoreResult<()> {
    if state.started {
        Ok(())
    } else {
        Err(streaming_error(
            "provider returned streaming data before message_start",
        ))
    }
}

fn update_usage(stream_usage: Option<&StreamUsage>, usage: &mut GenerationUsage) -> CoreResult<()> {
    if let Some(stream_usage) = stream_usage {
        merge_cumulative_usage(&mut usage.input_tokens, stream_usage.input_tokens)?;
        merge_cumulative_usage(
            &mut usage.cached_read_tokens,
            stream_usage.cache_read_input_tokens,
        )?;
        merge_cumulative_usage(
            &mut usage.cached_write_tokens,
            stream_usage.cache_creation_input_tokens,
        )?;
        merge_cumulative_usage(&mut usage.output_tokens, stream_usage.output_tokens)?;
        merge_cumulative_usage(
            &mut usage.reasoning_tokens,
            stream_usage
                .output_tokens_details
                .as_ref()
                .and_then(|details| details.thinking_tokens),
        )?;
        validate_raw_usage_counters(stream_usage, usage.provider_raw_summary.as_ref())?;
        usage.provider_raw_summary = merge_usage_summary(
            usage.provider_raw_summary.as_ref(),
            &[
                (
                    "cache_creation.ephemeral_5m_input_tokens",
                    stream_usage
                        .cache_creation
                        .as_ref()
                        .and_then(|details| details.ephemeral_5m_input_tokens),
                ),
                (
                    "cache_creation.ephemeral_1h_input_tokens",
                    stream_usage
                        .cache_creation
                        .as_ref()
                        .and_then(|details| details.ephemeral_1h_input_tokens),
                ),
            ],
        );
    }
    Ok(())
}

fn merge_cumulative_usage(current: &mut Option<u64>, next: Option<u64>) -> CoreResult<()> {
    if let Some(next) = next {
        if current.is_some_and(|current| next < current) {
            return Err(streaming_error(
                "provider returned decreasing cumulative usage",
            ));
        }
        *current = Some(next);
    }
    Ok(())
}

fn validate_raw_usage_counters(
    stream_usage: &StreamUsage,
    current: Option<&lorepia_domain::BoundedJson>,
) -> CoreResult<()> {
    let current = current
        .map(|summary| {
            serde_json::from_str::<serde_json::Value>(summary.as_str())
                .map_err(|_| streaming_error("provider returned invalid cumulative usage state"))
        })
        .transpose()?;
    for (key, next) in [
        (
            "cache_creation.ephemeral_5m_input_tokens",
            stream_usage
                .cache_creation
                .as_ref()
                .and_then(|details| details.ephemeral_5m_input_tokens),
        ),
        (
            "cache_creation.ephemeral_1h_input_tokens",
            stream_usage
                .cache_creation
                .as_ref()
                .and_then(|details| details.ephemeral_1h_input_tokens),
        ),
    ] {
        let previous = current
            .as_ref()
            .and_then(|summary| summary.get(key))
            .and_then(serde_json::Value::as_u64);
        if let (Some(previous), Some(next)) = (previous, next)
            && next < previous
        {
            return Err(streaming_error(
                "provider returned decreasing cumulative usage",
            ));
        }
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

fn ensure_event_stream_content_type(headers: &reqwest::header::HeaderMap) -> CoreResult<()> {
    let is_event_stream = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"));
    if is_event_stream {
        Ok(())
    } else {
        Err(streaming_error(
            "provider returned an unexpected content type",
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
    let (code, message) = if error.is_timeout() {
        (
            CoreErrorCode::ProviderUnavailable,
            "provider request timed out",
        )
    } else {
        (CoreErrorCode::NetworkUnavailable, "provider request failed")
    };
    CoreError::new(code, message, true)
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
        _ => (
            CoreErrorCode::ProviderUnavailable,
            status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT,
        ),
    };
    CoreError::new(
        code,
        format!("provider returned HTTP {}", status.as_u16()),
        recoverable,
    )
}

fn stream_error(kind: &str) -> CoreError {
    let (code, message, recoverable) = match kind {
        "invalid_request_error" | "request_too_large" => (
            CoreErrorCode::InvalidInput,
            "provider rejected the streaming request",
            false,
        ),
        "authentication_error" | "permission_error" => (
            CoreErrorCode::ProviderAuthFailed,
            "provider returned a streaming authentication error",
            false,
        ),
        "rate_limit_error" => (
            CoreErrorCode::ProviderRateLimited,
            "provider returned a streaming rate-limit error",
            true,
        ),
        "overloaded_error" | "api_error" | "timeout_error" => (
            CoreErrorCode::ProviderUnavailable,
            "provider returned a transient streaming error",
            true,
        ),
        _ => (
            CoreErrorCode::ProviderUnavailable,
            "provider returned a streaming error",
            false,
        ),
    };
    CoreError::new(code, message, recoverable)
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
        ConversationId, GenerationId, GenerationPresetId, GenerationProviderProvenance,
        GenerationRequest, Message, MessageRole, ModelRouteId, OpaqueReasoningContext,
    };
    use tokio::sync::{mpsc, watch};

    use super::*;
    use crate::parameter_mapping::{PromptCacheDirective, ProviderRequestPlan, RequestBodyPatch};

    fn request() -> GenerationRequest {
        let conversation_id = ConversationId::new();
        let mut system = Message::user(conversation_id.clone(), "system instructions");
        system.role = MessageRole::System;
        let user = Message::user(conversation_id.clone(), "hello");
        GenerationRequest {
            generation_id: GenerationId::new(),
            conversation_id,
            model: "claude-fixture".to_owned(),
            messages: vec![system, user],
            resolved_prompt_plan: None,
            provider_execution_plan_hash: None,
            temperature: Some(0.25),
            max_output_tokens: Some(321),
            provider_provenance: None,
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        }
    }

    fn max_tokens_plan(value: serde_json::Value) -> ProviderRequestPlan {
        ProviderRequestPlan {
            family: ApiFamily::AnthropicMessages,
            body_patches: vec![RequestBodyPatch {
                path: "max_tokens".to_owned(),
                value,
            }],
            prompt_cache: PromptCacheDirective::None,
            cache_dialect: crate::parameter_mapping::PromptCacheWireDialect::Unsupported,
            preserve_opaque_reasoning_state: false,
        }
    }

    fn thinking_budget_plan(budget_tokens: u64) -> ProviderRequestPlan {
        ProviderRequestPlan {
            family: ApiFamily::AnthropicMessages,
            body_patches: vec![
                RequestBodyPatch {
                    path: "thinking.type".to_owned(),
                    value: serde_json::json!("enabled"),
                },
                RequestBodyPatch {
                    path: "thinking.budget_tokens".to_owned(),
                    value: serde_json::json!(budget_tokens),
                },
            ],
            prompt_cache: PromptCacheDirective::None,
            cache_dialect: crate::parameter_mapping::PromptCacheWireDialect::Unsupported,
            preserve_opaque_reasoning_state: false,
        }
    }

    #[test]
    fn preserves_omitted_max_tokens_and_rejects_it_before_networking() {
        let mut request = request();
        request.max_output_tokens = None;
        let payload = serde_json::to_value(request_payload(request.clone()).expect("payload"))
            .expect("payload JSON");
        assert!(payload.get("max_tokens").is_none());

        let provider = AnthropicMessagesProvider::new("http://127.0.0.1:9", Duration::from_secs(1))
            .expect("provider");
        let client = Client::new();
        let error = provider
            .generation_request(&client, request, Some("synthetic-api-key"))
            .expect_err("missing max_tokens must fail before constructing a request");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            error.message,
            "Anthropic max_tokens must be an explicit positive integer"
        );
    }

    #[test]
    fn accepts_a_positive_planned_max_tokens_and_rejects_invalid_values() {
        let mut request = request();
        request.max_output_tokens = None;
        let provider = AnthropicMessagesProvider::new("http://127.0.0.1:9", Duration::from_secs(1))
            .expect("provider")
            .with_request_plan(max_tokens_plan(serde_json::json!(777)));
        let client = Client::new();
        let built = provider
            .generation_request(&client, request.clone(), Some("synthetic-api-key"))
            .expect("positive planned max_tokens")
            .build()
            .expect("HTTP request");
        let body: serde_json::Value = serde_json::from_slice(
            built
                .body()
                .and_then(reqwest::Body::as_bytes)
                .expect("JSON request body"),
        )
        .expect("request JSON");
        assert_eq!(body["max_tokens"], 777);

        for invalid in [
            serde_json::Value::Null,
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!("4096"),
        ] {
            let provider =
                AnthropicMessagesProvider::new("http://127.0.0.1:9", Duration::from_secs(1))
                    .expect("provider")
                    .with_request_plan(max_tokens_plan(invalid));
            let error = provider
                .generation_request(&client, request.clone(), Some("synthetic-api-key"))
                .expect_err("invalid planned max_tokens");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert_eq!(
                error.message,
                "Anthropic max_tokens must be an explicit positive integer"
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn omits_base_temperature_and_validates_budget_for_active_anthropic_thinking() {
        let client = Client::new();
        for kind in ["adaptive", "enabled"] {
            let provider =
                AnthropicMessagesProvider::new("http://127.0.0.1:9", Duration::from_secs(1))
                    .expect("provider")
                    .with_request_plan(ProviderRequestPlan {
                        family: ApiFamily::AnthropicMessages,
                        body_patches: vec![RequestBodyPatch {
                            path: "thinking.type".to_owned(),
                            value: serde_json::json!(kind),
                        }],
                        prompt_cache: PromptCacheDirective::None,
                        cache_dialect:
                            crate::parameter_mapping::PromptCacheWireDialect::Unsupported,
                        preserve_opaque_reasoning_state: false,
                    });
            let built = provider
                .generation_request(&client, request(), Some("synthetic-api-key"))
                .expect("active thinking request")
                .build()
                .expect("HTTP request");
            let body: serde_json::Value = serde_json::from_slice(
                built
                    .body()
                    .and_then(reqwest::Body::as_bytes)
                    .expect("request body"),
            )
            .expect("request JSON");
            assert_eq!(body["thinking"]["type"], kind);
            assert!(body.get("temperature").is_none());
        }

        let plain = AnthropicMessagesProvider::new("http://127.0.0.1:9", Duration::from_secs(1))
            .expect("provider")
            .generation_request(&client, request(), Some("synthetic-api-key"))
            .expect("plain request")
            .build()
            .expect("HTTP request");
        let plain: serde_json::Value = serde_json::from_slice(
            plain
                .body()
                .and_then(reqwest::Body::as_bytes)
                .expect("request body"),
        )
        .expect("request JSON");
        assert_eq!(plain["temperature"], 0.25);

        for budget_tokens in [321, 322] {
            let invalid_budget =
                AnthropicMessagesProvider::new("http://127.0.0.1:9", Duration::from_secs(1))
                    .expect("provider")
                    .with_request_plan(thinking_budget_plan(budget_tokens));
            let error = invalid_budget
                .generation_request(&client, request(), Some("synthetic-api-key"))
                .expect_err("budget must be less than final max_tokens");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert_eq!(
                error.message,
                "Anthropic thinking budget must be less than max_tokens"
            );
        }
        let valid_budget =
            AnthropicMessagesProvider::new("http://127.0.0.1:9", Duration::from_secs(1))
                .expect("provider")
                .with_request_plan(thinking_budget_plan(320));
        let valid_budget = valid_budget
            .generation_request(&client, request(), Some("synthetic-api-key"))
            .expect("budget one below max_tokens must be accepted")
            .build()
            .expect("valid budget request");
        let valid_budget: serde_json::Value = serde_json::from_slice(
            valid_budget
                .body()
                .and_then(reqwest::Body::as_bytes)
                .expect("valid budget body"),
        )
        .expect("valid budget JSON");
        assert_eq!(valid_budget["thinking"]["budget_tokens"], 320);

        let explicit_temperature =
            AnthropicMessagesProvider::new("http://127.0.0.1:9", Duration::from_secs(1))
                .expect("provider")
                .with_request_plan(ProviderRequestPlan {
                    family: ApiFamily::AnthropicMessages,
                    body_patches: vec![
                        RequestBodyPatch {
                            path: "thinking.type".to_owned(),
                            value: serde_json::json!("adaptive"),
                        },
                        RequestBodyPatch {
                            path: "temperature".to_owned(),
                            value: serde_json::json!(0.8),
                        },
                    ],
                    prompt_cache: PromptCacheDirective::None,
                    cache_dialect: crate::parameter_mapping::PromptCacheWireDialect::Unsupported,
                    preserve_opaque_reasoning_state: false,
                });
        let mut no_base_temperature = request();
        no_base_temperature.temperature = None;
        let error = explicit_temperature
            .generation_request(&client, no_base_temperature, Some("synthetic-api-key"))
            .expect_err("explicit thinking temperature must not be silently removed");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            error.message,
            "Anthropic thinking cannot be combined with an explicit temperature"
        );
    }

    #[test]
    fn constructs_only_safe_messages_endpoints() {
        let provider =
            AnthropicMessagesProvider::new("https://api.anthropic.com", Duration::from_secs(1))
                .expect("official origin");
        assert_eq!(
            provider.endpoint.as_str(),
            "https://api.anthropic.com/v1/messages"
        );

        for allowed in [
            "http://localhost:11434",
            "http://127.0.0.2:11434",
            "http://[::1]:11434",
        ] {
            assert!(
                AnthropicMessagesProvider::new(allowed, Duration::from_secs(1)).is_ok(),
                "{allowed}"
            );
        }

        for rejected in [
            "http://example.com",
            "ftp://api.anthropic.com",
            "https://user:secret@api.anthropic.com",
            "https://api.anthropic.com/prefix",
            "https://api.anthropic.com?key=secret",
            "https://api.anthropic.com#fragment",
        ] {
            assert!(
                AnthropicMessagesProvider::new(rejected, Duration::from_secs(1)).is_err(),
                "{rejected}"
            );
        }
        assert!(
            AnthropicMessagesProvider::new("https://api.anthropic.com", Duration::from_secs(0))
                .is_err()
        );
    }

    fn read_http_request(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
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

    fn stream_server(body: &[u8], fragment_bytes: usize) -> (String, std_mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stream server");
        let address = listener.local_addr().expect("stream server address");
        let chunks = body
            .chunks(fragment_bytes)
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>();
        let (captured, request) = std_mpsc::sync_channel(1);
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let incoming = read_http_request(&mut stream).expect("read stream request");
            captured.send(incoming).expect("capture request");
            if stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
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
        (format!("http://{address}"), request)
    }

    fn status_server(status: &str, body: &str, extra_headers: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind status server");
        let address = listener.local_addr().expect("status server address");
        let status = status.to_owned();
        let body = body.to_owned();
        let extra_headers = extra_headers.to_owned();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            read_http_request(&mut stream).expect("read status request");
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",
                body.len()
            )
            .expect("write status");
        });
        format!("http://{address}")
    }

    async fn generate_from_stream(
        body: &[u8],
        fragment_bytes: usize,
    ) -> (
        CoreResult<GenerationUsage>,
        Vec<ProviderEvent>,
        std_mpsc::Receiver<Vec<u8>>,
    ) {
        generate_request_from_stream(body, fragment_bytes, request()).await
    }

    async fn generate_request_from_stream(
        body: &[u8],
        fragment_bytes: usize,
        request: GenerationRequest,
    ) -> (
        CoreResult<GenerationUsage>,
        Vec<ProviderEvent>,
        std_mpsc::Receiver<Vec<u8>>,
    ) {
        let (base_url, captured) = stream_server(body, fragment_bytes);
        let provider =
            AnthropicMessagesProvider::new(&base_url, Duration::from_secs(10)).expect("provider");
        let (sink, mut events) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let result = provider
            .generate(request, Some("synthetic-api-key"), sink, cancelled)
            .await;
        let mut received = Vec::new();
        while let Ok(event) = events.try_recv() {
            received.push(event);
        }
        (result, received, captured)
    }

    #[tokio::test]
    async fn sends_messages_request_with_required_headers_and_system_field() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let (result, events, captured) = generate_from_stream(body.as_bytes(), 7).await;
        result.expect("valid stream");
        assert!(events.is_empty());

        let request = captured.recv().expect("captured request");
        let request = String::from_utf8(request).expect("UTF-8 request");
        let (headers, body) = request.split_once("\r\n\r\n").expect("request parts");
        assert!(headers.starts_with("POST /v1/messages HTTP/1.1\r\n"));
        let headers_lower = headers.to_ascii_lowercase();
        assert!(headers_lower.contains("x-api-key: synthetic-api-key\r\n"));
        assert!(headers_lower.contains("anthropic-version: 2023-06-01\r\n"));
        assert!(headers_lower.contains("accept: text/event-stream\r\n"));

        let body: serde_json::Value = serde_json::from_str(body).expect("request JSON");
        assert_eq!(body["model"], "claude-fixture");
        assert_eq!(body["system"], "system instructions");
        assert_eq!(body["max_tokens"], 321);
        assert_eq!(body["stream"], true);
        assert_eq!(
            body["messages"],
            serde_json::json!([{"role": "user", "content": "hello"}])
        );
    }

    #[tokio::test]
    async fn streams_fragmented_text_thinking_and_cumulative_usage() {
        let body = concat!(
            ": keepalive\r\n\r\n",
            "event: message_start\r\n",
            "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":13,\"cache_creation_input_tokens\":4,\"cache_read_input_tokens\":9,\"output_tokens\":1,\"output_tokens_details\":{\"thinking_tokens\":1},\"cache_creation\":{\"ephemeral_5m_input_tokens\":3,\"ephemeral_1h_input_tokens\":1}}}}\r\n\r\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"생각\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"synthetic-signature\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\r\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"안녕\"}}\r\n\r\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: future_event\n",
            "data: {\"type\":\"future_event\",\"new_field\":true}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7,\"output_tokens_details\":{\"thinking_tokens\":5}}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let (result, events, _captured) = generate_from_stream(body.as_bytes(), 1).await;

        let usage = result.expect("valid stream");
        assert_eq!(usage.input_tokens, Some(13));
        assert_eq!(usage.cached_read_tokens, Some(9));
        assert_eq!(usage.cached_write_tokens, Some(4));
        assert_eq!(usage.output_tokens, Some(7));
        assert_eq!(usage.reasoning_tokens, Some(5));
        assert_eq!(usage.tool_tokens, None);
        assert_eq!(
            usage
                .provider_raw_summary
                .as_ref()
                .map(lorepia_domain::BoundedJson::as_str),
            Some(
                r#"{"cache_creation.ephemeral_1h_input_tokens":1,"cache_creation.ephemeral_5m_input_tokens":3}"#
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
    async fn rejects_decreasing_cumulative_anthropic_usage() {
        let body = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[],\"usage\":{\"output_tokens\":0}}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":10,\"cache_read_input_tokens\":8}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":5,\"cache_read_input_tokens\":7}}\n\n",
        );
        let (result, events, _captured) = generate_from_stream(body.as_bytes(), 3).await;
        let error = result.expect_err("cumulative usage must not decrease");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(
            error.message,
            "provider returned decreasing cumulative usage"
        );
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn represents_anthropic_tool_use_as_bounded_inert_protocol_events() {
        let body = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"lookup\",\"input\":{}}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"lore\\\"}\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let (result, events, _captured) = generate_from_stream(body.as_bytes(), 2).await;
        result.expect("tool-use stream is represented, never executed");
        let id = ToolCallId::parse("toolu_1").expect("tool id");
        assert_eq!(
            events,
            vec![
                ProviderEvent::ToolCallStarted {
                    id: id.clone(),
                    name: ToolName::parse("lookup").expect("tool name"),
                },
                ProviderEvent::ToolCallArgumentsDelta {
                    id: id.clone(),
                    delta: ToolCallArgumentsDelta::parse(r#"{"query":"lore"}"#).expect("arguments"),
                },
                ProviderEvent::ToolCallCompleted { id },
            ]
        );
    }

    #[tokio::test]
    async fn rejects_preserved_tool_use_topology_without_exact_tool_results() {
        let body = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"private-thinking\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"private-signature\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_unsafe\",\"name\":\"lookup\",\"input\":{}}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let mut generation = request();
        generation.provider_provenance = Some(GenerationProviderProvenance {
            api_family: ApiFamily::AnthropicMessages,
            model_route_id: ModelRouteId::from("anthropic-route"),
            generation_preset_id: GenerationPresetId::from("anthropic-preset"),
        });
        generation.preserve_opaque_reasoning_state = true;

        let (result, events, _captured) =
            generate_request_from_stream(body.as_bytes(), 7, generation).await;
        let error = result.expect_err("unsafe tool-use topology");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(
            error.message,
            "provider returned an Anthropic tool-use topology without exact tool results"
        );
        assert!(!error.message.contains("private-"));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ProviderEvent::OpaqueReasoningState(_)))
        );
    }

    #[tokio::test]
    async fn accepts_anthropic_reasoning_topology_without_persisting_when_disabled() {
        let cases = [
            concat!(
                "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"opaque\"}}\n\n",
                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{}}\n\n",
                "data: {\"type\":\"message_stop\"}\n\n",
            ),
            concat!(
                "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"opaque\"}}\n\n",
                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{}}\n\n",
                "data: {\"type\":\"message_stop\"}\n\n",
            ),
        ];

        for body in cases {
            let (result, events, _captured) = generate_from_stream(body.as_bytes(), 3).await;
            result.expect("valid reasoning topology");
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, ProviderEvent::OpaqueReasoningState(_)))
            );
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn preserves_and_replays_the_exact_ordered_anthropic_content_topology() {
        let thinking_canary = "private-thinking-canary";
        let signature_canary = "private-signature-canary";
        let redacted_canary = "private-redacted-canary";
        let body = format!(
            concat!(
                "data: {{\"type\":\"message_start\",\"message\":{{\"role\":\"assistant\",\"content\":[]}}}}\n\n",
                "data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}}}\n\n",
                "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"thinking_delta\",\"thinking\":\"{}\"}}}}\n\n",
                "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"signature_delta\",\"signature\":\"{}\"}}}}\n\n",
                "data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n",
                "data: {{\"type\":\"content_block_start\",\"index\":1,\"content_block\":{{\"type\":\"redacted_thinking\",\"data\":\"{}\"}}}}\n\n",
                "data: {{\"type\":\"content_block_stop\",\"index\":1}}\n\n",
                "data: {{\"type\":\"content_block_start\",\"index\":2,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n",
                "data: {{\"type\":\"content_block_delta\",\"index\":2,\"delta\":{{\"type\":\"text_delta\",\"text\":\"answer\"}}}}\n\n",
                "data: {{\"type\":\"content_block_stop\",\"index\":2}}\n\n",
                "data: {{\"type\":\"message_delta\",\"delta\":{{}},\"usage\":{{}}}}\n\n",
                "data: {{\"type\":\"message_stop\"}}\n\n",
            ),
            thinking_canary, signature_canary, redacted_canary
        );
        let mut generation = request();
        generation.provider_provenance = Some(GenerationProviderProvenance {
            api_family: ApiFamily::AnthropicMessages,
            model_route_id: ModelRouteId::from("anthropic-route"),
            generation_preset_id: GenerationPresetId::from("anthropic-preset"),
        });
        generation.preserve_opaque_reasoning_state = true;

        let (result, events, _captured) =
            generate_request_from_stream(body.as_bytes(), 5, generation).await;
        result.expect("exact Anthropic topology");

        let expected_blocks = vec![
            AnthropicContentBlock::Thinking {
                thinking: AnthropicBlockText::parse(thinking_canary).expect("thinking"),
                signature: OpaqueReasoningData::parse(signature_canary).expect("signature"),
            },
            AnthropicContentBlock::RedactedThinking {
                data: OpaqueReasoningData::parse(redacted_canary).expect("redacted thinking"),
            },
            AnthropicContentBlock::Text {
                text: AnthropicBlockText::parse("answer").expect("text"),
            },
        ];
        let state = events
            .iter()
            .find_map(|event| match event {
                ProviderEvent::OpaqueReasoningState(state) => Some(state.clone()),
                ProviderEvent::TextDelta(_)
                | ProviderEvent::ReasoningDelta(_)
                | ProviderEvent::ToolCallStarted { .. }
                | ProviderEvent::ToolCallArgumentsDelta { .. }
                | ProviderEvent::ToolCallCompleted { .. } => None,
            })
            .expect("one opaque topology event");
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ProviderEvent::OpaqueReasoningState(_)))
                .count(),
            1
        );
        let OpaqueReasoningState::AnthropicMessages { content_blocks } = &state else {
            panic!("Anthropic topology state");
        };
        assert_eq!(content_blocks.blocks(), expected_blocks);
        let debug = format!("{state:?}");
        for canary in [thinking_canary, signature_canary, redacted_canary] {
            assert!(!debug.contains(canary));
        }

        let mut replay = request();
        let mut assistant = Message::user(replay.conversation_id.clone(), "answer");
        assistant.role = MessageRole::Assistant;
        replay.messages.push(assistant.clone());
        replay.provider_provenance = Some(GenerationProviderProvenance {
            api_family: ApiFamily::AnthropicMessages,
            model_route_id: ModelRouteId::from("anthropic-route"),
            generation_preset_id: GenerationPresetId::from("anthropic-preset"),
        });
        replay.preserve_opaque_reasoning_state = true;
        replay.opaque_reasoning_context = vec![OpaqueReasoningContext {
            source_message_id: assistant.id,
            api_family: ApiFamily::AnthropicMessages,
            model: replay.model.clone(),
            model_route_id: ModelRouteId::from("anthropic-route"),
            generation_preset_id: GenerationPresetId::from("anthropic-preset"),
            state,
        }];

        let generic = serde_json::to_string(&replay).expect("generic request JSON");
        for canary in [thinking_canary, signature_canary, redacted_canary] {
            assert!(!generic.contains(canary));
        }
        let payload =
            serde_json::to_value(request_payload(replay.clone()).expect("replay payload"))
                .expect("payload JSON");
        assert_eq!(
            payload["messages"][1]["content"],
            serde_json::json!([
                {
                    "type": "thinking",
                    "thinking": thinking_canary,
                    "signature": signature_canary
                },
                {"type": "redacted_thinking", "data": redacted_canary},
                {"type": "text", "text": "answer"}
            ])
        );

        replay.preserve_opaque_reasoning_state = false;
        replay.provider_provenance = None;
        let discarded =
            serde_json::to_value(request_payload(replay).expect("preserve=false request payload"))
                .expect("payload JSON");
        assert_eq!(discarded["messages"][1]["content"], "answer");
        for canary in [thinking_canary, signature_canary, redacted_canary] {
            assert!(!discarded.to_string().contains(canary));
        }
    }

    #[tokio::test]
    async fn rejects_cross_route_and_mutated_anthropic_replay_before_connecting() {
        let private_canary = "private-preflight-canary";
        let topology = AnthropicContentBlockTopology::new(vec![
            AnthropicContentBlock::Thinking {
                thinking: AnthropicBlockText::parse(private_canary).expect("thinking"),
                signature: OpaqueReasoningData::parse("signature").expect("signature"),
            },
            AnthropicContentBlock::Text {
                text: AnthropicBlockText::parse("answer").expect("text"),
            },
        ])
        .expect("topology");
        let mut base = request();
        let mut assistant = Message::user(base.conversation_id.clone(), "answer");
        assistant.role = MessageRole::Assistant;
        base.messages.push(assistant.clone());
        base.provider_provenance = Some(GenerationProviderProvenance {
            api_family: ApiFamily::AnthropicMessages,
            model_route_id: ModelRouteId::from("current-route"),
            generation_preset_id: GenerationPresetId::from("current-preset"),
        });
        base.preserve_opaque_reasoning_state = true;
        base.opaque_reasoning_context = vec![OpaqueReasoningContext {
            source_message_id: assistant.id,
            api_family: ApiFamily::AnthropicMessages,
            model: base.model.clone(),
            model_route_id: ModelRouteId::from("other-route"),
            generation_preset_id: GenerationPresetId::from("prior-preset"),
            state: OpaqueReasoningState::AnthropicMessages {
                content_blocks: topology,
            },
        }];

        let mut mutated = base.clone();
        mutated.opaque_reasoning_context[0].model_route_id = ModelRouteId::from("current-route");
        mutated.messages.last_mut().expect("assistant").content = "mutated answer".to_owned();

        let mut unsafe_tool = base.clone();
        unsafe_tool.opaque_reasoning_context[0].model_route_id =
            ModelRouteId::from("current-route");
        unsafe_tool.opaque_reasoning_context[0].state = OpaqueReasoningState::AnthropicMessages {
            content_blocks: AnthropicContentBlockTopology::new(vec![
                AnthropicContentBlock::Thinking {
                    thinking: AnthropicBlockText::parse(private_canary).expect("thinking"),
                    signature: OpaqueReasoningData::parse("signature").expect("signature"),
                },
                AnthropicContentBlock::Text {
                    text: AnthropicBlockText::parse("answer").expect("text"),
                },
                AnthropicContentBlock::ToolUse {
                    id: ToolCallId::parse("toolu_unsafe").expect("tool id"),
                    name: ToolName::parse("lookup").expect("tool name"),
                    input: AnthropicToolInput::from_value(&serde_json::json!({}))
                        .expect("tool input"),
                },
            ])
            .expect("unsafe topology"),
        };

        for generation in [base, mutated, unsafe_tool] {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind zero-connect listener");
            listener
                .set_nonblocking(true)
                .expect("nonblocking listener");
            let provider = AnthropicMessagesProvider::new(
                &format!(
                    "http://{}",
                    listener.local_addr().expect("listener address")
                ),
                Duration::from_secs(1),
            )
            .expect("provider");
            let (sink, _events) = mpsc::channel(4);
            let (_cancel, cancelled) = watch::channel(false);

            let error = provider
                .generate(generation, Some("synthetic-api-key"), sink, cancelled)
                .await
                .expect_err("invalid replay must fail before networking");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert!(!error.message.contains(private_canary));
            let accept_error = listener.accept().expect_err("must not connect");
            assert_eq!(accept_error.kind(), io::ErrorKind::WouldBlock);
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn rejects_invalid_anthropic_topologies_without_exposing_private_fragments() {
        let oversized_signature = "s".repeat(MAX_OPAQUE_REASONING_ITEM_BYTES + 1);
        let cases = [
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"private-missing-signature\"}}\n\n",
                    "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                )
                .to_owned(),
                "provider returned invalid thinking signature",
                "private-missing-signature",
            ),
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"private-start-signature\"}}\n\n",
                )
                .to_owned(),
                "provider returned an invalid thinking block start",
                "private-start-signature",
            ),
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"private-start-text\"}}\n\n",
                )
                .to_owned(),
                "provider returned a non-empty text block start",
                "private-start-text",
            ),
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"private-start-thinking\",\"signature\":\"\"}}\n\n",
                )
                .to_owned(),
                "provider returned a non-empty thinking block start",
                "private-start-thinking",
            ),
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"private-first-signature\"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"private-second-signature\"}}\n\n",
                )
                .to_owned(),
                "provider returned an invalid thinking signature sequence",
                "private-second-signature",
            ),
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"private-signature\"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"private-after-signature\"}}\n\n",
                )
                .to_owned(),
                "provider returned thinking data after its signature",
                "private-after-signature",
            ),
            (
                "data: {\"type\":\"message_start\",\"message\":{\"content\":[]}}\n\n".to_owned(),
                "provider returned an invalid Anthropic message_start",
                "private-absent-canary",
            ),
            (
                "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"private-start-content\"}]}}\n\n".to_owned(),
                "provider returned an invalid Anthropic message_start",
                "private-start-content",
            ),
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"message_stop\"}\n\n",
                )
                .to_owned(),
                "provider returned message_stop before message_delta",
                "private-absent-canary",
            ),
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"message_delta\",\"usage\":{}}\n\n",
                )
                .to_owned(),
                "provider returned malformed streaming data",
                "private-absent-canary",
            ),
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"message_delta\",\"delta\":\"private-invalid-delta\",\"usage\":{}}\n\n",
                )
                .to_owned(),
                "provider returned malformed streaming data",
                "private-invalid-delta",
            ),
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"future_private_block\",\"data\":\"private-unknown\"}}\n\n",
                )
                .to_owned(),
                "provider returned an unsupported content block",
                "private-unknown",
            ),
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"private-noncontiguous\"}}\n\n",
                )
                .to_owned(),
                "provider returned a non-contiguous Anthropic content-block index",
                "private-noncontiguous",
            ),
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\",\"future\":\"private-block-extra\"}}\n\n",
                )
                .to_owned(),
                "provider returned malformed streaming data",
                "private-block-extra",
            ),
            (
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"\",\"future\":\"private-delta-extra\"}}\n\n",
                )
                .to_owned(),
                "provider returned malformed streaming data",
                "private-delta-extra",
            ),
            (
                format!(
                    concat!(
                        "data: {{\"type\":\"message_start\",\"message\":{{\"role\":\"assistant\",\"content\":[]}}}}\n\n",
                        "data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}}}\n\n",
                        "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"signature_delta\",\"signature\":\"{}\"}}}}\n\n",
                    ),
                    oversized_signature
                ),
                "provider Anthropic opaque block exceeded its safety limit",
                oversized_signature.as_str(),
            ),
        ];

        for (body, expected, canary) in cases {
            let (result, _events, _captured) = generate_from_stream(body.as_bytes(), 1024).await;
            let error = result.expect_err("invalid topology must fail closed");
            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
            assert_eq!(error.message, expected);
            assert!(!error.message.contains(canary));
        }
    }

    #[tokio::test]
    async fn rejects_incomplete_malformed_and_oversized_streams() {
        let cases = [
            (
                b"".as_slice(),
                "provider returned an empty streaming response",
            ),
            (
                b"data: {not-json}\n\n".as_slice(),
                "provider returned malformed streaming data",
            ),
            (
                b"data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n"
                    .as_slice(),
                "provider stream ended before message_stop",
            ),
            (
                b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"early\"}}\n\n"
                    .as_slice(),
                "provider returned streaming data before message_start",
            ),
        ];

        for (body, expected) in cases {
            let (result, events, _captured) = generate_from_stream(body, 3).await;
            let error = result.expect_err("stream must fail");
            assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
            assert_eq!(error.message, expected);
            assert!(events.is_empty());
        }

        let mut oversized = b"data: ".to_vec();
        oversized.extend(std::iter::repeat_n(
            b'x',
            MAX_SSE_EVENT_BYTES + 1 - oversized.len(),
        ));
        let (result, events, _captured) = generate_from_stream(&oversized, 64 * 1024).await;
        let error = result.expect_err("oversized event must fail");
        assert_eq!(error.message, "provider streaming event exceeded 1 MiB");
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn classifies_http_errors_without_exposing_response_bodies_or_keys() {
        for (status, code, recoverable) in [
            ("400 Bad Request", CoreErrorCode::InvalidInput, false),
            ("401 Unauthorized", CoreErrorCode::ProviderAuthFailed, false),
            (
                "429 Too Many Requests",
                CoreErrorCode::ProviderRateLimited,
                true,
            ),
            (
                "529 Site Overloaded",
                CoreErrorCode::ProviderUnavailable,
                true,
            ),
        ] {
            let provider = AnthropicMessagesProvider::new(
                &status_server(
                    status,
                    r#"{"error":{"message":"synthetic-api-key leaked in body"}}"#,
                    "",
                ),
                Duration::from_secs(2),
            )
            .expect("provider");
            let (sink, _events) = mpsc::channel(4);
            let (_cancel, cancelled) = watch::channel(false);
            let error = provider
                .generate(request(), Some("synthetic-api-key"), sink, cancelled)
                .await
                .expect_err("status must fail");

            assert_eq!(error.code, code);
            assert_eq!(error.recoverable, recoverable);
            assert!(!error.message.contains("synthetic-api-key"));
            assert!(!error.message.contains("leaked in body"));
        }
    }

    #[tokio::test]
    async fn classifies_stream_errors_without_exposing_provider_messages() {
        for (kind, code, recoverable) in [
            ("invalid_request_error", CoreErrorCode::InvalidInput, false),
            (
                "authentication_error",
                CoreErrorCode::ProviderAuthFailed,
                false,
            ),
            ("rate_limit_error", CoreErrorCode::ProviderRateLimited, true),
            ("overloaded_error", CoreErrorCode::ProviderUnavailable, true),
            ("future_error", CoreErrorCode::ProviderUnavailable, false),
        ] {
            let body = format!(
                "event: error\ndata: {{\"type\":\"error\",\"error\":{{\"type\":\"{kind}\",\"message\":\"secret synthetic-api-key\"}}}}\n\n"
            );
            let (result, events, _captured) = generate_from_stream(body.as_bytes(), 2).await;
            let error = result.expect_err("stream error must fail");

            assert_eq!(error.code, code);
            assert_eq!(error.recoverable, recoverable);
            assert!(!error.message.contains("synthetic-api-key"));
            assert!(!error.message.contains("secret"));
            assert!(events.is_empty());
        }
    }

    #[tokio::test]
    async fn refuses_redirects_and_unexpected_success_content_types() {
        let redirect = AnthropicMessagesProvider::new(
            &status_server(
                "302 Found",
                "",
                "Location: https://example.invalid/collect\r\n",
            ),
            Duration::from_secs(2),
        )
        .expect("provider");
        let (sink, _events) = mpsc::channel(4);
        let (_cancel, cancelled) = watch::channel(false);
        let error = redirect
            .generate(request(), Some("synthetic-api-key"), sink, cancelled)
            .await
            .expect_err("redirect must not be followed");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(error.message, "provider returned HTTP 302");

        let wrong_content_type = AnthropicMessagesProvider::new(
            &status_server("200 OK", r#"{"type":"message"}"#, ""),
            Duration::from_secs(2),
        )
        .expect("provider");
        let (sink, _events) = mpsc::channel(4);
        let (_cancel, cancelled) = watch::channel(false);
        let error = wrong_content_type
            .generate(request(), Some("synthetic-api-key"), sink, cancelled)
            .await
            .expect_err("JSON success body must not be parsed as SSE");
        assert_eq!(
            error.message,
            "provider returned an unexpected content type"
        );
    }

    #[tokio::test]
    async fn cancellation_interrupts_buffered_event_backpressure() {
        let body = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"first\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"second\"}}\n\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let (base_url, _captured) = stream_server(body.as_bytes(), body.len());
        let provider =
            AnthropicMessagesProvider::new(&base_url, Duration::from_secs(2)).expect("provider");
        let (sink, mut events) = mpsc::channel(1);
        let (cancel, cancelled) = watch::channel(false);
        let generation = tokio::spawn(async move {
            provider
                .generate(request(), Some("synthetic-api-key"), sink, cancelled)
                .await
        });

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
            .expect_err("cancellation must interrupt event backpressure");
        assert_eq!(error.code, CoreErrorCode::Cancelled);
        assert_eq!(
            events.try_recv().expect("first event"),
            ProviderEvent::TextDelta("first".to_owned())
        );
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn validates_credentials_and_observes_precancel_without_networking() {
        let provider = AnthropicMessagesProvider::new("http://127.0.0.1:9", Duration::from_secs(1))
            .expect("provider");
        let (sink, _events) = mpsc::channel(4);
        let (_cancel, cancelled) = watch::channel(false);
        let error = provider
            .generate(request(), None, sink, cancelled)
            .await
            .expect_err("missing credential");
        assert_eq!(error.code, CoreErrorCode::ProviderAuthFailed);

        let (sink, _events) = mpsc::channel(4);
        let (_cancel, cancelled) = watch::channel(false);
        let invalid = "synthetic-secret\ninjected";
        let error = provider
            .generate(request(), Some(invalid), sink, cancelled)
            .await
            .expect_err("invalid header credential");
        assert_eq!(error.code, CoreErrorCode::ProviderAuthFailed);
        assert!(!error.message.contains(invalid));

        let (sink, _events) = mpsc::channel(4);
        let (cancel, cancelled) = watch::channel(false);
        cancel.send(true).expect("cancel");
        let error = provider
            .generate(request(), Some("synthetic-api-key"), sink, cancelled)
            .await
            .expect_err("precancelled");
        assert_eq!(error.code, CoreErrorCode::Cancelled);
    }
}
