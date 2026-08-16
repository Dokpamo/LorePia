use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use lorepia_domain::{
    AnthropicContentBlock, ApiFamily, CoreError, CoreErrorCode, CoreResult, GenerationRequest,
    GenerationUsage, OpaqueReasoningState, ToolCallArgumentsDelta, ToolCallId, ToolName,
    validate_opaque_reasoning_states,
};
use lorepia_providers::{Provider, ProviderEvent};
use tokio::sync::{mpsc, watch};
use zeroize::{Zeroize, Zeroizing};

use crate::{ChatEvent, ChatEventKind};

/// Maximum cumulative UTF-8 bytes accepted from text and reasoning deltas.
pub const MAX_GENERATED_OUTPUT_BYTES: usize = 256 * 1024;
/// Maximum cumulative Unicode scalars accepted from text and reasoning deltas.
pub const MAX_GENERATED_OUTPUT_CHARS: usize = 64 * 1024;
/// Maximum number of inert tool calls represented by one generation.
pub const MAX_GENERATED_TOOL_CALLS: usize = 128;
/// Maximum number of provider protocol events consumed by one generation.
pub const MAX_GENERATED_PROVIDER_EVENTS: usize = 8_192;
/// Maximum cumulative UTF-8 size of tool-call argument fragments.
pub const MAX_GENERATED_TOOL_ARGUMENT_BYTES: usize = 256 * 1024;
/// Maximum cumulative Unicode scalar count of tool-call argument fragments.
pub const MAX_GENERATED_TOOL_ARGUMENT_CHARS: usize = 128 * 1024;
/// Stable failure text emitted when a provider ignores the output safety bound.
pub const OUTPUT_LIMIT_ERROR_MESSAGE: &str =
    "provider output exceeded the 262144-byte or 65536-character safety limit";
/// Maximum UTF-8 size of an error message accepted from a provider.
const MAX_PROVIDER_ERROR_BYTES: usize = 16 * 1024;
/// Maximum Unicode scalar count of an error message accepted from a provider.
const MAX_PROVIDER_ERROR_CHARS: usize = 4 * 1024;
/// Maximum credential size accepted by the borrowed reflection guard.
const MAX_BORROWED_CREDENTIAL_BYTES: usize = 16 * 1024;
/// Stable failure text used instead of any provider-reflected credential.
pub const CREDENTIAL_REFLECTION_ERROR_MESSAGE: &str =
    "provider response contained protected credential material";
/// Stable input error for credentials too large to guard with bounded memory.
const CREDENTIAL_LIMIT_ERROR_MESSAGE: &str =
    "provider credential exceeds the 16384-byte safety limit";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationOutcome {
    pub text: String,
    pub usage: GenerationUsage,
    pub opaque_reasoning_state: Vec<OpaqueReasoningState>,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationFailure {
    pub error: CoreError,
    pub partial_text: String,
    pub last_sequence: u64,
}

#[derive(Clone)]
struct CredentialReflectionGuard<'credential> {
    credential: Option<&'credential str>,
    prefix_table: Option<Arc<[usize]>>,
}

impl<'credential> CredentialReflectionGuard<'credential> {
    fn new(credential: Option<&'credential str>) -> Self {
        let credential = credential.filter(|credential| !credential.is_empty());
        Self {
            credential,
            prefix_table: credential
                .map(build_credential_prefix_table)
                .map(Arc::<[usize]>::from),
        }
    }

    fn is_enabled(&self) -> bool {
        self.credential.is_some()
    }

    fn contains(&self, value: &str) -> bool {
        self.credential
            .is_some_and(|credential| value.contains(credential))
    }

    fn validate_tool_start(
        &self,
        id: ToolCallId,
        name: ToolName,
    ) -> CoreResult<(ToolCallId, ToolName)> {
        if !self.contains(id.as_str()) && !self.contains(name.as_str()) {
            return Ok((id, name));
        }
        let mut id = id.into_inner();
        let mut name = name.into_inner();
        id.zeroize();
        name.zeroize();
        Err(credential_reflection_error())
    }

    fn validate_tool_id(&self, id: ToolCallId) -> CoreResult<ToolCallId> {
        if !self.contains(id.as_str()) {
            return Ok(id);
        }
        let mut id = id.into_inner();
        id.zeroize();
        Err(credential_reflection_error())
    }

    fn validate_opaque_state(
        &self,
        mut state: OpaqueReasoningState,
    ) -> CoreResult<OpaqueReasoningState> {
        let Some(credential) = self.credential else {
            return Ok(state);
        };
        if opaque_state_contains_exact(&state, credential) {
            state.zeroize_sensitive_payloads();
            return Err(credential_reflection_error());
        }
        Ok(state)
    }

    fn validate_usage(&self, mut usage: GenerationUsage) -> CoreResult<GenerationUsage> {
        let reflected = self.credential.is_some_and(|credential| {
            usage
                .provider_raw_summary
                .as_ref()
                .is_some_and(|summary| bounded_json_contains_exact(summary.as_str(), credential))
        });
        if !reflected {
            return Ok(usage);
        }
        if let Some(summary) = usage.provider_raw_summary.take() {
            let mut summary = summary.into_inner();
            summary.zeroize();
        }
        Err(credential_reflection_error())
    }
}

struct ReflectionStream<'credential> {
    guard: CredentialReflectionGuard<'credential>,
    pending: Zeroizing<String>,
}

impl<'credential> ReflectionStream<'credential> {
    fn new(guard: CredentialReflectionGuard<'credential>) -> Self {
        Self {
            guard,
            pending: Zeroizing::new(String::new()),
        }
    }

    fn push(&mut self, mut fragment: String) -> CoreResult<String> {
        let Some(credential) = self.guard.credential else {
            return Ok(fragment);
        };

        if self.pending.is_empty() {
            std::mem::swap(&mut *self.pending, &mut fragment);
        } else {
            self.pending.push_str(&fragment);
            fragment.zeroize();
        }

        let prefix_table = self
            .guard
            .prefix_table
            .as_deref()
            .expect("enabled credential guard has a prefix table");
        let Some(retained_bytes) =
            credential_suffix_after_linear_scan(&self.pending, credential, prefix_table)
        else {
            self.pending.zeroize();
            return Err(credential_reflection_error());
        };

        let release_end = self.pending.len().saturating_sub(retained_bytes);
        if release_end == 0 {
            return Ok(String::new());
        }
        let retained = self.pending.split_off(release_end);
        Ok(std::mem::replace(&mut *self.pending, retained))
    }

    fn finish(&mut self) -> String {
        std::mem::take(&mut *self.pending)
    }
}

struct GenerationAccumulator<'credential> {
    generation_id: lorepia_domain::GenerationId,
    conversation_id: lorepia_domain::ConversationId,
    credential_guard: CredentialReflectionGuard<'credential>,
    text_reflection: ReflectionStream<'credential>,
    reasoning_reflection: ReflectionStream<'credential>,
    text: String,
    output_bytes: usize,
    output_chars: usize,
    tool_argument_bytes: usize,
    tool_argument_chars: usize,
    active_tool_calls: BTreeSet<ToolCallId>,
    completed_tool_calls: BTreeSet<ToolCallId>,
    tool_argument_reflections: BTreeMap<ToolCallId, ReflectionStream<'credential>>,
    pending_tool_completions: Vec<(ToolCallId, ReflectionStream<'credential>)>,
    provider_event_count: usize,
    preserve_opaque_reasoning_state: bool,
    provider_family: Option<ApiFamily>,
    opaque_reasoning_state: Vec<OpaqueReasoningState>,
    sequence: u64,
}

impl GenerationAccumulator<'_> {
    fn failure(&self, error: CoreError) -> GenerationFailure {
        GenerationFailure {
            error,
            partial_text: self.text.clone(),
            last_sequence: self.sequence,
        }
    }

    async fn forward(
        &mut self,
        event: ProviderEvent,
        events: &mpsc::Sender<ChatEvent>,
    ) -> CoreResult<()> {
        self.provider_event_count = self
            .provider_event_count
            .checked_add(1)
            .ok_or_else(provider_event_limit_error)?;
        if self.provider_event_count > MAX_GENERATED_PROVIDER_EVENTS {
            zeroize_rejected_provider_event(event);
            return Err(provider_event_limit_error());
        }

        match event {
            ProviderEvent::TextDelta(mut delta) => {
                if let Err(error) = self.track_output_delta(&delta) {
                    delta.zeroize();
                    return Err(error);
                }
                let filtered = self.text_reflection.push(delta)?;
                if !filtered.is_empty() || !self.credential_guard.is_enabled() {
                    self.text.push_str(&filtered);
                    self.emit(ChatEventKind::TextDelta(filtered), events)
                        .await?;
                }
            }
            ProviderEvent::ReasoningDelta(mut delta) => {
                if let Err(error) = self.track_output_delta(&delta) {
                    delta.zeroize();
                    return Err(error);
                }
                let filtered = self.reasoning_reflection.push(delta)?;
                if !filtered.is_empty() || !self.credential_guard.is_enabled() {
                    self.emit(ChatEventKind::ReasoningDelta(filtered), events)
                        .await?;
                }
            }
            ProviderEvent::ToolCallStarted { id, name } => {
                self.forward_tool_start(id, name, events).await?;
            }
            ProviderEvent::ToolCallArgumentsDelta { id, delta } => {
                self.forward_tool_arguments(id, delta, events).await?;
            }
            ProviderEvent::ToolCallCompleted { id } => {
                self.complete_tool_call(id)?;
            }
            ProviderEvent::OpaqueReasoningState(state) => {
                let state = self.credential_guard.validate_opaque_state(state)?;
                if self.preserve_opaque_reasoning_state {
                    let state = validate_opaque_state_family(state, self.provider_family)?;
                    self.opaque_reasoning_state.push(state);
                    if let Err(error) =
                        validate_opaque_reasoning_states(&self.opaque_reasoning_state)
                    {
                        if let Some(mut state) = self.opaque_reasoning_state.pop() {
                            state.zeroize_sensitive_payloads();
                        }
                        return Err(CoreError::new(
                            CoreErrorCode::ProviderUnavailable,
                            error,
                            false,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    async fn forward_tool_start(
        &mut self,
        id: ToolCallId,
        name: ToolName,
        events: &mpsc::Sender<ChatEvent>,
    ) -> CoreResult<()> {
        let (id, name) = self.credential_guard.validate_tool_start(id, name)?;
        if self.active_tool_calls.len() + self.completed_tool_calls.len()
            >= MAX_GENERATED_TOOL_CALLS
        {
            return Err(tool_protocol_error(
                "provider exceeded the 128 tool-call generation limit",
            ));
        }
        if self.active_tool_calls.contains(&id) || self.completed_tool_calls.contains(&id) {
            return Err(tool_protocol_error(
                "provider reused a tool-call identifier",
            ));
        }
        self.active_tool_calls.insert(id.clone());
        self.tool_argument_reflections.insert(
            id.clone(),
            ReflectionStream::new(self.credential_guard.clone()),
        );
        self.emit(ChatEventKind::ToolCallStarted { id, name }, events)
            .await
    }

    async fn forward_tool_arguments(
        &mut self,
        id: ToolCallId,
        delta: ToolCallArgumentsDelta,
        events: &mpsc::Sender<ChatEvent>,
    ) -> CoreResult<()> {
        let id = self.credential_guard.validate_tool_id(id)?;
        let mut delta = delta.into_inner();
        if !self.active_tool_calls.contains(&id) {
            let reflected = self.credential_guard.contains(&delta);
            delta.zeroize();
            if reflected {
                return Err(credential_reflection_error());
            }
            return Err(tool_protocol_error(
                "provider sent tool-call arguments outside an active call",
            ));
        }
        if let Err(error) = self.track_tool_arguments(&delta) {
            delta.zeroize();
            return Err(error);
        }
        let filtered = self
            .tool_argument_reflections
            .get_mut(&id)
            .ok_or_else(|| {
                tool_protocol_error("provider tool-call argument stream was unavailable")
            })?
            .push(delta)?;
        if filtered.is_empty() {
            return Ok(());
        }
        self.emit_filtered_tool_arguments(id, filtered, events)
            .await
    }

    fn complete_tool_call(&mut self, id: ToolCallId) -> CoreResult<()> {
        let id = self.credential_guard.validate_tool_id(id)?;
        if !self.active_tool_calls.remove(&id) {
            return Err(tool_protocol_error(
                "provider completed a tool call that was not active",
            ));
        }
        let reflection = self.tool_argument_reflections.remove(&id).ok_or_else(|| {
            tool_protocol_error("provider tool-call argument stream was unavailable")
        })?;
        self.completed_tool_calls.insert(id.clone());
        self.pending_tool_completions.push((id, reflection));
        Ok(())
    }

    async fn finish_public_streams(&mut self, events: &mpsc::Sender<ChatEvent>) -> CoreResult<()> {
        let text = self.text_reflection.finish();
        if !text.is_empty() {
            let mut text_for_state = text.clone();
            if let Err(error) = self.emit(ChatEventKind::TextDelta(text), events).await {
                text_for_state.zeroize();
                return Err(error);
            }
            self.text.push_str(&text_for_state);
            text_for_state.zeroize();
        }
        let reasoning = self.reasoning_reflection.finish();
        if !reasoning.is_empty() {
            self.emit(ChatEventKind::ReasoningDelta(reasoning), events)
                .await?;
        }
        for (id, mut reflection) in std::mem::take(&mut self.pending_tool_completions) {
            let filtered = reflection.finish();
            if !filtered.is_empty() {
                self.emit_filtered_tool_arguments(id.clone(), filtered, events)
                    .await?;
            }
            self.emit(ChatEventKind::ToolCallCompleted { id }, events)
                .await?;
        }
        Ok(())
    }

    async fn emit_filtered_tool_arguments(
        &mut self,
        id: ToolCallId,
        filtered: String,
        events: &mpsc::Sender<ChatEvent>,
    ) -> CoreResult<()> {
        let mut remaining = Zeroizing::new(filtered);
        while !remaining.is_empty() {
            let end = bounded_tool_argument_chunk_end(&remaining);
            let tail = remaining.split_off(end);
            let chunk = std::mem::replace(&mut *remaining, tail);
            let chunk = ToolCallArgumentsDelta::parse(chunk)
                .map_err(|_| tool_protocol_error("provider tool-call argument filtering failed"))?;
            self.emit(
                ChatEventKind::ToolCallArgumentsDelta {
                    id: id.clone(),
                    delta: chunk,
                },
                events,
            )
            .await?;
        }
        Ok(())
    }

    async fn emit(
        &mut self,
        kind: ChatEventKind,
        events: &mpsc::Sender<ChatEvent>,
    ) -> CoreResult<()> {
        self.sequence = self.sequence.saturating_add(1);
        send_event(
            events,
            ChatEvent::new(
                self.generation_id.clone(),
                self.conversation_id.clone(),
                self.sequence,
                kind,
            ),
        )
        .await
    }

    fn track_output_delta(&mut self, delta: &str) -> CoreResult<()> {
        let next_bytes = self
            .output_bytes
            .checked_add(delta.len())
            .ok_or_else(output_limit_error)?;
        let next_chars = self
            .output_chars
            .checked_add(delta.chars().count())
            .ok_or_else(output_limit_error)?;
        if next_bytes > MAX_GENERATED_OUTPUT_BYTES || next_chars > MAX_GENERATED_OUTPUT_CHARS {
            return Err(output_limit_error());
        }
        self.output_bytes = next_bytes;
        self.output_chars = next_chars;
        Ok(())
    }

    fn track_tool_arguments(&mut self, delta: &str) -> CoreResult<()> {
        let next_bytes = self
            .tool_argument_bytes
            .checked_add(delta.len())
            .ok_or_else(tool_arguments_limit_error)?;
        let next_chars = self
            .tool_argument_chars
            .checked_add(delta.chars().count())
            .ok_or_else(tool_arguments_limit_error)?;
        if next_bytes > MAX_GENERATED_TOOL_ARGUMENT_BYTES
            || next_chars > MAX_GENERATED_TOOL_ARGUMENT_CHARS
        {
            return Err(tool_arguments_limit_error());
        }
        self.tool_argument_bytes = next_bytes;
        self.tool_argument_chars = next_chars;
        Ok(())
    }

    fn ensure_protocol_complete(&self) -> CoreResult<()> {
        if self.active_tool_calls.is_empty() {
            Ok(())
        } else {
            Err(tool_protocol_error(
                "provider generation ended with an incomplete tool call",
            ))
        }
    }
}

pub async fn run_generation(
    provider: &dyn Provider,
    request: GenerationRequest,
    credential: Option<&str>,
    events: mpsc::Sender<ChatEvent>,
    cancelled: watch::Receiver<bool>,
) -> Result<GenerationOutcome, GenerationFailure> {
    if let Err(error) = validate_borrowed_credential(credential) {
        return Err(GenerationFailure {
            error,
            partial_text: String::new(),
            last_sequence: 1,
        });
    }
    let credential_guard = CredentialReflectionGuard::new(credential);
    let mut state = GenerationAccumulator {
        generation_id: request.generation_id.clone(),
        conversation_id: request.conversation_id.clone(),
        credential_guard: credential_guard.clone(),
        text_reflection: ReflectionStream::new(credential_guard.clone()),
        reasoning_reflection: ReflectionStream::new(credential_guard),
        text: String::new(),
        output_bytes: 0,
        output_chars: 0,
        tool_argument_bytes: 0,
        tool_argument_chars: 0,
        active_tool_calls: BTreeSet::new(),
        completed_tool_calls: BTreeSet::new(),
        tool_argument_reflections: BTreeMap::new(),
        pending_tool_completions: Vec::new(),
        provider_event_count: 0,
        preserve_opaque_reasoning_state: request.preserve_opaque_reasoning_state,
        provider_family: request
            .provider_provenance
            .as_ref()
            .map(|provenance| provenance.api_family),
        opaque_reasoning_state: Vec::new(),
        sequence: 1,
    };
    crate::prompt::validate_generation_request(&request).map_err(|error| state.failure(error))?;
    send_event(
        &events,
        ChatEvent::new(
            state.generation_id.clone(),
            state.conversation_id.clone(),
            state.sequence,
            ChatEventKind::GenerationStarted,
        ),
    )
    .await
    .map_err(|error| state.failure(error))?;

    let result = collect_provider_events(
        provider, request, credential, &events, cancelled, &mut state,
    )
    .await?;
    match result {
        Ok(usage) => {
            let usage = state
                .credential_guard
                .validate_usage(usage)
                .map_err(|error| state.failure(error))?;
            state
                .finish_public_streams(&events)
                .await
                .map_err(|error| state.failure(error))?;
            state.sequence = state.sequence.saturating_add(1);
            send_event(
                &events,
                ChatEvent::new(
                    state.generation_id.clone(),
                    state.conversation_id.clone(),
                    state.sequence,
                    ChatEventKind::UsageUpdated(usage.clone()),
                ),
            )
            .await
            .map_err(|error| state.failure(error))?;
            Ok(GenerationOutcome {
                text: state.text,
                usage,
                opaque_reasoning_state: state.opaque_reasoning_state,
                last_sequence: state.sequence,
            })
        }
        Err(error) => Err(GenerationFailure {
            error,
            partial_text: state.text,
            last_sequence: state.sequence,
        }),
    }
}

async fn collect_provider_events(
    provider: &dyn Provider,
    request: GenerationRequest,
    credential: Option<&str>,
    events: &mpsc::Sender<ChatEvent>,
    cancelled: watch::Receiver<bool>,
    state: &mut GenerationAccumulator<'_>,
) -> Result<CoreResult<GenerationUsage>, GenerationFailure> {
    if *cancelled.borrow() {
        return Ok(Err(cancelled_error()));
    }
    let mut cancellation = cancelled.clone();
    let mut cancellation_open = true;
    let (provider_sender, mut provider_events) = mpsc::channel(64);
    let generation = provider.generate(request, credential, provider_sender, cancelled);
    tokio::pin!(generation);
    let mut provider_open = true;
    let result = loop {
        tokio::select! {
            changed = cancellation.changed(), if cancellation_open => {
                match changed {
                    Ok(()) if *cancellation.borrow_and_update() => break Err(cancelled_error()),
                    Ok(()) => {}
                    Err(_) => cancellation_open = false,
                }
            }
            event = provider_events.recv(), if provider_open => {
                match event {
                    Some(event) => state
                        .forward(event, events)
                        .await
                        .map_err(|error| state.failure(error))?,
                    None => provider_open = false,
                }
            }
            result = &mut generation => break result,
        }
    };
    let result = result.map_err(|error| bound_provider_error(error, &state.credential_guard));

    // Completion and the final delta may share one scheduler tick.
    // Every send awaited by the provider is already visible in the queue when
    // its future completes. Do not wait for unrelated retained sender clones:
    // provider completion is the protocol boundary for its stream.
    while let Ok(event) = provider_events.try_recv() {
        state
            .forward(event, events)
            .await
            .map_err(|error| state.failure(error))?;
    }
    if result.is_ok() {
        state
            .ensure_protocol_complete()
            .map_err(|error| state.failure(error))?;
    }
    Ok(result)
}

fn validate_borrowed_credential(credential: Option<&str>) -> CoreResult<()> {
    if credential.is_some_and(|credential| credential.len() > MAX_BORROWED_CREDENTIAL_BYTES) {
        return Err(CoreError::invalid(CREDENTIAL_LIMIT_ERROR_MESSAGE));
    }
    Ok(())
}

fn build_credential_prefix_table(credential: &str) -> Vec<usize> {
    let bytes = credential.as_bytes();
    let mut table = vec![0; bytes.len()];
    let mut matched = 0;
    for index in 1..bytes.len() {
        while matched > 0 && bytes[index] != bytes[matched] {
            matched = table[matched - 1];
        }
        if bytes[index] == bytes[matched] {
            matched += 1;
        }
        table[index] = matched;
    }
    table
}

/// Returns the longest proper credential prefix at the end of `value`.
///
/// A full match returns `None`. Both inputs are valid UTF-8, so a match that
/// reaches the end of `value` also ends at a scalar boundary.
fn credential_suffix_after_linear_scan(
    value: &str,
    credential: &str,
    prefix_table: &[usize],
) -> Option<usize> {
    let pattern = credential.as_bytes();
    debug_assert!(!pattern.is_empty());
    debug_assert_eq!(pattern.len(), prefix_table.len());

    let mut matched = 0;
    for byte in value.bytes() {
        while matched > 0 && byte != pattern[matched] {
            matched = prefix_table[matched - 1];
        }
        if byte == pattern[matched] {
            matched += 1;
            if matched == pattern.len() {
                return None;
            }
        }
    }
    debug_assert!(credential.is_char_boundary(matched));
    debug_assert!(value.is_char_boundary(value.len().saturating_sub(matched)));
    Some(matched)
}

fn bounded_tool_argument_chunk_end(value: &str) -> usize {
    let mut end = 0;
    for (characters, (index, character)) in value.char_indices().enumerate() {
        let next_end = index + character.len_utf8();
        if characters == lorepia_domain::MAX_TOOL_ARGUMENT_DELTA_CHARS
            || next_end > lorepia_domain::MAX_TOOL_ARGUMENT_DELTA_BYTES
        {
            break;
        }
        end = next_end;
    }
    end
}

fn validate_opaque_state_family(
    mut state: OpaqueReasoningState,
    provider_family: Option<ApiFamily>,
) -> CoreResult<OpaqueReasoningState> {
    let compatible = matches!(
        (provider_family, &state),
        (
            Some(ApiFamily::OpenAiResponses),
            OpaqueReasoningState::OpenAiResponses { .. }
        ) | (
            Some(ApiFamily::OpenAiChatCompletions),
            OpaqueReasoningState::OpenRouterReasoning { .. }
        ) | (
            Some(ApiFamily::AnthropicMessages),
            OpaqueReasoningState::AnthropicMessages { .. }
        ) | (
            Some(ApiFamily::GeminiGenerateContent),
            OpaqueReasoningState::GeminiThoughtSignature { .. }
        )
    );
    if compatible {
        return Ok(state);
    }
    state.zeroize_sensitive_payloads();
    Err(CoreError::new(
        CoreErrorCode::ProviderUnavailable,
        "provider returned opaque reasoning state for a different API family",
        false,
    ))
}

fn zeroize_rejected_provider_event(event: ProviderEvent) {
    match event {
        ProviderEvent::TextDelta(mut delta) | ProviderEvent::ReasoningDelta(mut delta) => {
            delta.zeroize();
        }
        ProviderEvent::ToolCallStarted { id, name } => {
            let mut id = id.into_inner();
            let mut name = name.into_inner();
            id.zeroize();
            name.zeroize();
        }
        ProviderEvent::ToolCallArgumentsDelta { id, delta } => {
            let mut id = id.into_inner();
            let mut delta = delta.into_inner();
            id.zeroize();
            delta.zeroize();
        }
        ProviderEvent::ToolCallCompleted { id } => {
            let mut id = id.into_inner();
            id.zeroize();
        }
        ProviderEvent::OpaqueReasoningState(mut state) => state.zeroize_sensitive_payloads(),
    }
}

fn opaque_state_contains_exact(state: &OpaqueReasoningState, credential: &str) -> bool {
    match state {
        OpaqueReasoningState::OpenAiResponses { item } => {
            item.contains_exact_for_reflection_guard(credential)
        }
        OpaqueReasoningState::GeminiThoughtSignature { signature, .. } => {
            signature.expose_to_provider().contains(credential)
        }
        OpaqueReasoningState::OpenRouterReasoning { topology } => {
            topology.contains_exact_for_reflection_guard(credential)
        }
        OpaqueReasoningState::AnthropicMessages { content_blocks } => {
            content_blocks.blocks().iter().any(|block| match block {
                AnthropicContentBlock::Text { text } => {
                    text.expose_to_provider().contains(credential)
                }
                AnthropicContentBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    thinking.expose_to_provider().contains(credential)
                        || signature.expose_to_provider().contains(credential)
                }
                AnthropicContentBlock::RedactedThinking { data } => {
                    data.expose_to_provider().contains(credential)
                }
                AnthropicContentBlock::ToolUse { id, name, input } => {
                    id.as_str().contains(credential)
                        || name.as_str().contains(credential)
                        || json_value_contains_exact(input.expose_to_provider(), credential)
                }
            })
        }
    }
}

fn bounded_json_contains_exact(encoded: &str, credential: &str) -> bool {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(encoded) else {
        return true;
    };
    let reflected = json_value_contains_exact(&value, credential);
    zeroize_json_strings(&mut value);
    reflected
}

fn json_value_contains_exact(value: &serde_json::Value, credential: &str) -> bool {
    match value {
        serde_json::Value::String(value) => value.contains(credential),
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| json_value_contains_exact(value, credential)),
        serde_json::Value::Object(values) => values.iter().any(|(key, value)| {
            key.contains(credential) || json_value_contains_exact(value, credential)
        }),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            false
        }
    }
}

fn zeroize_json_strings(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(value) => value.zeroize(),
        serde_json::Value::Array(values) => {
            for value in values {
                zeroize_json_strings(value);
            }
        }
        serde_json::Value::Object(values) => {
            for (mut key, mut value) in std::mem::take(values) {
                key.zeroize();
                zeroize_json_strings(&mut value);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn credential_reflection_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::ProviderUnavailable,
        CREDENTIAL_REFLECTION_ERROR_MESSAGE,
        false,
    )
}

fn output_limit_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::ProviderUnavailable,
        OUTPUT_LIMIT_ERROR_MESSAGE,
        false,
    )
}

fn cancelled_error() -> CoreError {
    CoreError::new(CoreErrorCode::Cancelled, "generation was cancelled", true)
}

fn tool_arguments_limit_error() -> CoreError {
    tool_protocol_error(
        "provider tool-call arguments exceeded the 262144-byte or 131072-character safety limit",
    )
}

fn provider_event_limit_error() -> CoreError {
    tool_protocol_error("provider exceeded the 8192-event generation limit")
}

fn bound_provider_error(
    mut error: CoreError,
    credential_guard: &CredentialReflectionGuard<'_>,
) -> CoreError {
    if credential_guard.contains(&error.message) || credential_guard.contains(&error.operation_id) {
        error.message.zeroize();
        error.operation_id.zeroize();
        return credential_reflection_error();
    }
    if error.message.len() <= MAX_PROVIDER_ERROR_BYTES
        && error.message.chars().count() <= MAX_PROVIDER_ERROR_CHARS
    {
        return error;
    }
    error.message.zeroize();
    CoreError::new(
        CoreErrorCode::ProviderUnavailable,
        "provider returned an oversized failure response",
        false,
    )
}

fn tool_protocol_error(message: &'static str) -> CoreError {
    CoreError::new(CoreErrorCode::ProviderUnavailable, message, false)
}

async fn send_event(events: &mpsc::Sender<ChatEvent>, event: ChatEvent) -> CoreResult<()> {
    match events.send(event).await {
        Ok(()) => Ok(()),
        Err(error) => {
            zeroize_unsent_chat_event(error.0);
            Err(lorepia_domain::CoreError::internal(
                "chat event receiver closed",
            ))
        }
    }
}

fn zeroize_unsent_chat_event(event: ChatEvent) {
    match event.kind {
        ChatEventKind::ReasoningDelta(mut delta) | ChatEventKind::TextDelta(mut delta) => {
            delta.zeroize();
        }
        ChatEventKind::ToolCallStarted { id, name } => {
            let mut id = id.into_inner();
            let mut name = name.into_inner();
            id.zeroize();
            name.zeroize();
        }
        ChatEventKind::ToolCallArgumentsDelta { id, delta } => {
            let mut id = id.into_inner();
            let mut delta = delta.into_inner();
            id.zeroize();
            delta.zeroize();
        }
        ChatEventKind::ToolCallCompleted { id } => {
            let mut id = id.into_inner();
            id.zeroize();
        }
        ChatEventKind::UsageUpdated(mut usage) => {
            if let Some(summary) = usage.provider_raw_summary.take() {
                let mut summary = summary.into_inner();
                summary.zeroize();
            }
        }
        ChatEventKind::GenerationFailed {
            mut code,
            mut message,
        } => {
            code.zeroize();
            message.zeroize();
        }
        ChatEventKind::GenerationStarted
        | ChatEventKind::MessageCommitted { .. }
        | ChatEventKind::GenerationCancelled
        | ChatEventKind::GenerationFinished => {}
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use async_trait::async_trait;
    use lorepia_domain::{
        AnthropicBlockText, AnthropicContentBlock, AnthropicContentBlockTopology,
        AnthropicToolInput, BoundedJson, ConversationId, GenerationId, GenerationPresetId,
        GenerationProviderProvenance, GenerationRequest, GenerationUsage, Message, MessageRole,
        ModelRouteId, OpaqueReasoningData, OpaqueReasoningState, OpenAiResponsesReasoningItem,
        OpenRouterReasoningDetail, OpenRouterReasoningTopology, ProviderCapabilities,
        ToolCallArgumentsDelta, ToolCallId, ToolName,
    };
    use lorepia_providers::{ProviderEventSender, StaticProvider};
    use tokio::{sync::Notify, time};

    use super::*;

    #[tokio::test]
    async fn emits_monotonic_sequence_and_collects_text() {
        let provider = StaticProvider::new("Hello");
        let (events, mut receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let request = GenerationRequest {
            generation_id: GenerationId::new(),
            conversation_id: ConversationId::new(),
            model: "static".to_owned(),
            messages: Vec::new(),
            temperature: Some(1.0),
            max_output_tokens: None,
            resolved_prompt_plan: None,
            provider_execution_plan_hash: None,
            provider_provenance: None,
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        };

        let outcome = run_generation(&provider, request, None, events, cancelled)
            .await
            .expect("generation");
        assert_eq!(outcome.text, "Hello");
        assert!(outcome.last_sequence >= 2);

        let mut sequences = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            sequences.push(event.sequence);
        }
        assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    }

    struct OpaqueStateProvider {
        states: Vec<OpaqueReasoningState>,
    }

    #[async_trait]
    impl Provider for OpaqueStateProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: true,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            for state in &self.states {
                sink.send(ProviderEvent::OpaqueReasoningState(state.clone()))
                    .await
                    .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            }
            Ok(GenerationUsage::default())
        }
    }

    #[tokio::test]
    async fn opaque_reasoning_state_is_internal_and_respects_preservation_setting() {
        let canary = "chat-opaque-state-canary";
        let state = OpaqueReasoningState::GeminiThoughtSignature {
            part_index: 0,
            signature: OpaqueReasoningData::parse(canary).expect("signature"),
        };
        let provider = OpaqueStateProvider {
            states: vec![state.clone()],
        };
        let mut preserved_request = request();
        preserved_request.preserve_opaque_reasoning_state = true;
        set_provider_family(&mut preserved_request, ApiFamily::GeminiGenerateContent);
        let (events, mut receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let outcome = run_generation(&provider, preserved_request, None, events, cancelled)
            .await
            .expect("preserved generation");
        assert_eq!(outcome.opaque_reasoning_state, vec![state.clone()]);
        assert!(!format!("{outcome:?}").contains(canary));
        while let Ok(event) = receiver.try_recv() {
            assert!(!format!("{event:?}").contains(canary));
        }

        let discarded_request = request();
        let (events, _receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let outcome = run_generation(&provider, discarded_request, None, events, cancelled)
            .await
            .expect("discarded generation");
        assert!(outcome.opaque_reasoning_state.is_empty());
    }

    #[tokio::test]
    async fn opaque_reasoning_state_count_is_bounded_before_persistence() {
        let states = (0..=lorepia_domain::MAX_OPAQUE_REASONING_STATE_COUNT)
            .map(|part_index| OpaqueReasoningState::GeminiThoughtSignature {
                part_index: u32::try_from(part_index).expect("part index"),
                signature: OpaqueReasoningData::parse(format!("state-{part_index}"))
                    .expect("signature"),
            })
            .collect::<Vec<_>>();

        let mut preserved_request = request();
        preserved_request.preserve_opaque_reasoning_state = true;
        set_provider_family(&mut preserved_request, ApiFamily::GeminiGenerateContent);
        let exact_provider = OpaqueStateProvider {
            states: states[..lorepia_domain::MAX_OPAQUE_REASONING_STATE_COUNT].to_vec(),
        };
        let (events, _receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let outcome = run_generation(
            &exact_provider,
            preserved_request.clone(),
            None,
            events,
            cancelled,
        )
        .await
        .expect("exact opaque-state count");
        assert_eq!(
            outcome.opaque_reasoning_state.len(),
            lorepia_domain::MAX_OPAQUE_REASONING_STATE_COUNT
        );

        let overflow_provider = OpaqueStateProvider { states };
        let (events, _receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let failure = run_generation(
            &overflow_provider,
            preserved_request,
            None,
            events,
            cancelled,
        )
        .await
        .expect_err("opaque-state overflow must fail");
        assert_eq!(failure.error.code, CoreErrorCode::ProviderUnavailable);
        assert!(!failure.error.recoverable);
        assert_eq!(
            failure.error.message,
            "opaque reasoning state exceeds the 32-item limit"
        );
    }

    #[tokio::test]
    async fn opaque_reasoning_serialized_envelope_is_rejected_before_outcome() {
        let escaped_signature = "\\".repeat(lorepia_domain::MAX_OPAQUE_REASONING_ITEM_BYTES - 128);
        let states = (0..4)
            .map(|part_index| {
                ProviderEvent::OpaqueReasoningState(OpaqueReasoningState::GeminiThoughtSignature {
                    part_index,
                    signature: OpaqueReasoningData::parse(escaped_signature.clone())
                        .expect("bounded escaped signature"),
                })
            })
            .collect();
        let (result, events) = run_scripted_for_family(
            "credential-not-in-state",
            states,
            Ok(GenerationUsage::default()),
            true,
            Some(ApiFamily::GeminiGenerateContent),
        )
        .await;
        let failure = result.expect_err("serialized opaque-state overflow must fail");
        assert_eq!(failure.error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(
            failure.error.message,
            format!(
                "opaque reasoning state exceeds the {}-byte serialized JSON limit",
                lorepia_domain::MAX_OPAQUE_REASONING_SERIALIZED_BYTES
            )
        );
        assert!(
            events
                .iter()
                .all(|event| !matches!(event.kind, ChatEventKind::UsageUpdated(_)))
        );
    }

    struct ChunkProvider {
        chunks: Vec<String>,
    }

    #[async_trait]
    impl Provider for ChunkProvider {
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
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            for chunk in &self.chunks {
                sink.send(ProviderEvent::TextDelta(chunk.clone()))
                    .await
                    .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            }
            Ok(GenerationUsage::default())
        }
    }

    fn request() -> GenerationRequest {
        GenerationRequest {
            generation_id: GenerationId::new(),
            conversation_id: ConversationId::new(),
            model: "static".to_owned(),
            messages: Vec::new(),
            temperature: Some(1.0),
            max_output_tokens: Some(4_096),
            resolved_prompt_plan: None,
            provider_execution_plan_hash: None,
            provider_provenance: None,
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        }
    }

    fn set_provider_family(request: &mut GenerationRequest, api_family: ApiFamily) {
        request.provider_provenance = Some(GenerationProviderProvenance {
            api_family,
            model_route_id: ModelRouteId::new(),
            generation_preset_id: GenerationPresetId::new(),
        });
    }

    #[tokio::test]
    async fn rejects_unbounded_or_privilege_escalating_requests_before_provider_dispatch() {
        let provider = StaticProvider::new("must not run");

        let mut too_many = request();
        too_many.messages = vec![
            Message::user(too_many.conversation_id.clone(), "bounded");
            crate::MAX_PROMPT_MESSAGES + 1
        ];
        let (events, mut receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let failure = run_generation(&provider, too_many, None, events, cancelled)
            .await
            .expect_err("message count must be validated before dispatch");
        assert_eq!(
            failure.error.message,
            "provider request exceeds the 128-message limit"
        );
        assert!(receiver.try_recv().is_err());

        let mut oversized = request();
        oversized.messages = vec![Message::user(
            oversized.conversation_id.clone(),
            "x".repeat(crate::MAX_PROMPT_BYTES + 1),
        )];
        let (events, mut receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let failure = run_generation(&provider, oversized, None, events, cancelled)
            .await
            .expect_err("prompt bytes must be validated before dispatch");
        assert_eq!(
            failure.error.message,
            "prompt exceeds the 524288-byte or 131072-character limit"
        );
        assert!(receiver.try_recv().is_err());

        let mut misplaced_system = request();
        let user = Message::user(misplaced_system.conversation_id.clone(), "hello");
        let mut system = Message::user(
            misplaced_system.conversation_id.clone(),
            "untrusted system injection",
        );
        system.role = MessageRole::System;
        misplaced_system.messages = vec![user, system];
        let (events, mut receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let failure = run_generation(&provider, misplaced_system, None, events, cancelled)
            .await
            .expect_err("late system message must not reach a provider");
        assert_eq!(
            failure.error.message,
            "provider request system message must be unique and first"
        );
        assert!(receiver.try_recv().is_err());
    }

    struct CancellationIgnoringProvider;

    #[async_trait]
    impl Provider for CancellationIgnoringProvider {
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
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            sink.send(ProviderEvent::TextDelta("partial".to_owned()))
                .await
                .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn cancellation_stops_a_provider_that_does_not_observe_its_receiver() {
        let (events, mut receiver) = mpsc::channel(16);
        let (cancel, cancelled) = watch::channel(false);
        let mut generation = tokio::spawn(async move {
            run_generation(
                &CancellationIgnoringProvider,
                request(),
                None,
                events,
                cancelled,
            )
            .await
        });

        loop {
            let event = receiver.recv().await.expect("generation event");
            if matches!(event.kind, ChatEventKind::TextDelta(_)) {
                break;
            }
        }
        cancel.send(true).expect("cancel active generation");

        let result = time::timeout(Duration::from_secs(1), &mut generation).await;
        let Ok(result) = result else {
            generation.abort();
            panic!("chat did not enforce cancellation");
        };
        let failure = result
            .expect("generation task")
            .expect_err("generation must be cancelled");
        assert_eq!(failure.error.code, CoreErrorCode::Cancelled);
        assert_eq!(failure.error.message, "generation was cancelled");
        assert!(failure.error.recoverable);
        assert_eq!(failure.partial_text, "partial");
    }

    struct RetainedSenderProvider {
        retained: Arc<Mutex<Option<ProviderEventSender>>>,
    }

    #[async_trait]
    impl Provider for RetainedSenderProvider {
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
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            self.retained
                .lock()
                .expect("retained sender lock")
                .replace(sink.clone());
            sink.send(ProviderEvent::TextDelta("complete".to_owned()))
                .await
                .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            Ok(GenerationUsage::default())
        }
    }

    #[tokio::test]
    async fn provider_completion_does_not_wait_for_retained_sender_clones() {
        let retained = Arc::new(Mutex::new(None));
        let provider = RetainedSenderProvider {
            retained: Arc::clone(&retained),
        };
        let (events, _receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);

        let outcome = time::timeout(
            Duration::from_secs(1),
            run_generation(&provider, request(), None, events, cancelled),
        )
        .await
        .expect("provider completion must be a stream boundary")
        .expect("completed generation");
        retained.lock().expect("retained sender lock").take();

        assert_eq!(outcome.text, "complete");
    }

    struct ExactUsageProvider {
        usage: GenerationUsage,
    }

    #[async_trait]
    impl Provider for ExactUsageProvider {
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
            _sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            Ok(self.usage.clone())
        }
    }

    #[tokio::test]
    async fn preserves_every_usage_field_without_narrowing_or_rewriting() {
        let usage = GenerationUsage {
            input_tokens: Some(u64::MAX),
            cached_read_tokens: Some(u64::MAX - 1),
            cached_write_tokens: Some(u64::MAX - 2),
            output_tokens: Some(u64::MAX - 3),
            reasoning_tokens: Some(u64::MAX - 4),
            tool_tokens: Some(u64::MAX - 5),
            provider_raw_summary: Some(
                BoundedJson::parse(r#"{"accepted_prediction_tokens":7,"audio_tokens":11}"#)
                    .expect("bounded usage summary"),
            ),
        };
        let provider = ExactUsageProvider {
            usage: usage.clone(),
        };
        let (events, mut receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);

        let outcome = run_generation(&provider, request(), None, events, cancelled)
            .await
            .expect("generation with usage");
        assert_eq!(outcome.usage, usage);

        let emitted_usage = std::iter::from_fn(|| receiver.try_recv().ok()).find_map(|event| {
            if let ChatEventKind::UsageUpdated(usage) = event.kind {
                Some(usage)
            } else {
                None
            }
        });
        assert_eq!(emitted_usage, Some(usage));
    }

    struct ExactErrorProvider {
        error: CoreError,
    }

    #[async_trait]
    impl Provider for ExactErrorProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: true,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            sink.send(ProviderEvent::ReasoningDelta("reasoning".to_owned()))
                .await
                .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            sink.send(ProviderEvent::TextDelta("partial".to_owned()))
                .await
                .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            Err(self.error.clone())
        }
    }

    #[tokio::test]
    async fn preserves_provider_errors_and_only_text_in_the_partial_response() {
        let error = CoreError::new(
            CoreErrorCode::ProviderRateLimited,
            "provider retry window is active",
            true,
        );
        let provider = ExactErrorProvider {
            error: error.clone(),
        };
        let (events, _receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);

        let failure = run_generation(&provider, request(), None, events, cancelled)
            .await
            .expect_err("provider error must remain a generation failure");

        assert_eq!(failure.error, error);
        assert_eq!(failure.partial_text, "partial");
        assert_eq!(failure.last_sequence, 3);
    }

    #[tokio::test]
    async fn replaces_oversized_provider_errors_before_they_can_become_chat_events() {
        let provider = ExactErrorProvider {
            error: CoreError::new(
                CoreErrorCode::ProviderAuthFailed,
                "x".repeat(MAX_PROVIDER_ERROR_BYTES + 1),
                true,
            ),
        };
        let (events, _receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);

        let failure = run_generation(&provider, request(), None, events, cancelled)
            .await
            .expect_err("oversized provider error must fail closed");

        assert_eq!(failure.error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(
            failure.error.message,
            "provider returned an oversized failure response"
        );
        assert!(!failure.error.recoverable);
        assert_eq!(failure.partial_text, "partial");
    }

    #[tokio::test]
    async fn accepts_output_at_the_exact_multibyte_utf8_boundary() {
        let output = "😀".repeat(MAX_GENERATED_OUTPUT_CHARS);
        assert_eq!(output.len(), MAX_GENERATED_OUTPUT_BYTES);
        let provider = ChunkProvider {
            chunks: vec![output.clone()],
        };
        let (events, _receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);

        let outcome = run_generation(&provider, request(), None, events, cancelled)
            .await
            .expect("exact boundary");
        assert_eq!(outcome.text, output);
    }

    #[tokio::test]
    async fn rejects_complete_multibyte_scalar_over_limit_and_preserves_safe_partial() {
        let safe_partial = "😀".repeat(MAX_GENERATED_OUTPUT_CHARS);
        let provider = ChunkProvider {
            chunks: vec![safe_partial.clone(), "😀".to_owned()],
        };
        let (events, mut receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);

        let failure = run_generation(&provider, request(), None, events, cancelled)
            .await
            .expect_err("provider must be bounded");
        assert_eq!(failure.error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(failure.error.message, OUTPUT_LIMIT_ERROR_MESSAGE);
        assert!(!failure.error.recoverable);
        assert_eq!(failure.partial_text, safe_partial);
        assert_eq!(failure.partial_text.len(), MAX_GENERATED_OUTPUT_BYTES);

        let forwarded_text = std::iter::from_fn(|| receiver.try_recv().ok())
            .filter_map(|event| match event.kind {
                ChatEventKind::TextDelta(delta) => Some(delta),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(forwarded_text, failure.partial_text);
    }

    #[tokio::test]
    async fn reasoning_and_text_share_one_cumulative_output_budget() {
        struct MixedProvider;

        #[async_trait]
        impl Provider for MixedProvider {
            fn capabilities(&self) -> ProviderCapabilities {
                ProviderCapabilities {
                    streaming: true,
                    reasoning: true,
                    max_context_tokens: None,
                }
            }

            async fn generate(
                &self,
                _request: GenerationRequest,
                _credential: Option<&str>,
                sink: ProviderEventSender,
                _cancelled: watch::Receiver<bool>,
            ) -> CoreResult<GenerationUsage> {
                sink.send(ProviderEvent::ReasoningDelta(
                    "😀".repeat(MAX_GENERATED_OUTPUT_CHARS - 1),
                ))
                .await
                .map_err(|_| CoreError::internal("provider event receiver closed"))?;
                sink.send(ProviderEvent::TextDelta("😀".to_owned()))
                    .await
                    .map_err(|_| CoreError::internal("provider event receiver closed"))?;
                sink.send(ProviderEvent::TextDelta("x".to_owned()))
                    .await
                    .map_err(|_| CoreError::internal("provider event receiver closed"))?;
                Ok(GenerationUsage::default())
            }
        }

        let (events, _receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let failure = run_generation(&MixedProvider, request(), None, events, cancelled)
            .await
            .expect_err("combined deltas must be bounded");
        assert_eq!(failure.error.message, OUTPUT_LIMIT_ERROR_MESSAGE);
        assert_eq!(failure.partial_text, "😀");
    }

    struct ProtocolProvider {
        protocol_events: Vec<ProviderEvent>,
    }

    #[async_trait]
    impl Provider for ProtocolProvider {
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
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            for event in &self.protocol_events {
                sink.send(event.clone())
                    .await
                    .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            }
            Ok(GenerationUsage::default())
        }
    }

    struct ScriptedProvider {
        protocol_events: Vec<ProviderEvent>,
        result: CoreResult<GenerationUsage>,
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: true,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            for event in &self.protocol_events {
                sink.send(event.clone())
                    .await
                    .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            }
            self.result.clone()
        }
    }

    async fn run_scripted(
        credential: &str,
        protocol_events: Vec<ProviderEvent>,
        result: CoreResult<GenerationUsage>,
        preserve_opaque_reasoning_state: bool,
    ) -> (Result<GenerationOutcome, GenerationFailure>, Vec<ChatEvent>) {
        run_scripted_for_family(
            credential,
            protocol_events,
            result,
            preserve_opaque_reasoning_state,
            None,
        )
        .await
    }

    async fn run_scripted_for_family(
        credential: &str,
        protocol_events: Vec<ProviderEvent>,
        result: CoreResult<GenerationUsage>,
        preserve_opaque_reasoning_state: bool,
        provider_family: Option<ApiFamily>,
    ) -> (Result<GenerationOutcome, GenerationFailure>, Vec<ChatEvent>) {
        let provider = ScriptedProvider {
            protocol_events,
            result,
        };
        let mut generation_request = request();
        generation_request.preserve_opaque_reasoning_state = preserve_opaque_reasoning_state;
        if let Some(provider_family) = provider_family {
            set_provider_family(&mut generation_request, provider_family);
        }
        let (events, mut receiver) = mpsc::channel(128);
        let (_cancel, cancelled) = watch::channel(false);
        let result = run_generation(
            &provider,
            generation_request,
            Some(credential),
            events,
            cancelled,
        )
        .await;
        let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect();
        (result, events)
    }

    fn assert_reflection_failure(
        result: Result<GenerationOutcome, GenerationFailure>,
        events: &[ChatEvent],
        credential: &str,
    ) -> GenerationFailure {
        let failure = result.expect_err("credential reflection must fail closed");
        assert_eq!(failure.error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(failure.error.message, CREDENTIAL_REFLECTION_ERROR_MESSAGE);
        assert!(!failure.error.recoverable);
        assert!(!failure.partial_text.contains(credential));
        assert!(!format!("{failure:?}").contains(credential));
        assert!(
            events
                .iter()
                .all(|event| !chat_event_contains_exact(event, credential))
        );
        let encoded_events = serde_json::to_string(events).expect("serialize chat events");
        assert!(!encoded_events.contains(credential));
        failure
    }

    fn chat_event_contains_exact(event: &ChatEvent, credential: &str) -> bool {
        match &event.kind {
            ChatEventKind::ReasoningDelta(delta) | ChatEventKind::TextDelta(delta) => {
                delta.contains(credential)
            }
            ChatEventKind::ToolCallStarted { id, name } => {
                id.as_str().contains(credential) || name.as_str().contains(credential)
            }
            ChatEventKind::ToolCallArgumentsDelta { id, delta } => {
                id.as_str().contains(credential) || delta.as_str().contains(credential)
            }
            ChatEventKind::ToolCallCompleted { id } => id.as_str().contains(credential),
            ChatEventKind::UsageUpdated(usage) => usage
                .provider_raw_summary
                .as_ref()
                .is_some_and(|summary| bounded_json_contains_exact(summary.as_str(), credential)),
            ChatEventKind::GenerationFailed { message, .. } => message.contains(credential),
            ChatEventKind::GenerationStarted
            | ChatEventKind::MessageCommitted { .. }
            | ChatEventKind::GenerationCancelled
            | ChatEventKind::GenerationFinished => false,
        }
    }

    fn tool_id() -> ToolCallId {
        ToolCallId::parse("call-1").expect("valid tool call id")
    }

    fn tool_protocol_with_fragments(fragments: Vec<String>) -> Vec<ProviderEvent> {
        let mut events = Vec::with_capacity(fragments.len() + 2);
        events.push(ProviderEvent::ToolCallStarted {
            id: tool_id(),
            name: ToolName::parse("lookup").expect("valid tool name"),
        });
        events.extend(fragments.into_iter().map(|fragment| {
            ProviderEvent::ToolCallArgumentsDelta {
                id: tool_id(),
                delta: ToolCallArgumentsDelta::parse(fragment).expect("bounded fragment"),
            }
        }));
        events.push(ProviderEvent::ToolCallCompleted { id: tool_id() });
        events
    }

    #[tokio::test]
    async fn forwards_bounded_tool_call_protocol_without_executing_it() {
        let provider = ProtocolProvider {
            protocol_events: vec![
                ProviderEvent::ToolCallStarted {
                    id: tool_id(),
                    name: ToolName::parse("lookup").expect("valid tool name"),
                },
                ProviderEvent::ToolCallArgumentsDelta {
                    id: tool_id(),
                    delta: ToolCallArgumentsDelta::parse(r#"{"q":"seoul"}"#)
                        .expect("valid arguments"),
                },
                ProviderEvent::ToolCallCompleted { id: tool_id() },
            ],
        };
        let (events, mut receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);

        let outcome = run_generation(&provider, request(), None, events, cancelled)
            .await
            .expect("complete tool protocol");
        assert!(outcome.text.is_empty());

        let kinds = std::iter::from_fn(|| receiver.try_recv().ok())
            .map(|event| event.kind)
            .collect::<Vec<_>>();
        assert!(matches!(kinds[0], ChatEventKind::GenerationStarted));
        assert!(matches!(kinds[1], ChatEventKind::ToolCallStarted { .. }));
        assert!(matches!(
            kinds[2],
            ChatEventKind::ToolCallArgumentsDelta { .. }
        ));
        assert!(matches!(kinds[3], ChatEventKind::ToolCallCompleted { .. }));
        assert!(matches!(kinds[4], ChatEventKind::UsageUpdated(_)));
    }

    #[tokio::test]
    async fn enforces_cumulative_tool_argument_byte_and_character_limits() {
        let character_fragment = "a".repeat(lorepia_domain::MAX_TOOL_ARGUMENT_DELTA_CHARS);
        let exact_characters = ProtocolProvider {
            protocol_events: tool_protocol_with_fragments(vec![character_fragment.clone(); 4]),
        };
        let (events, _receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        run_generation(&exact_characters, request(), None, events, cancelled)
            .await
            .expect("exact cumulative tool-argument character limit");

        let over_characters = ProtocolProvider {
            protocol_events: tool_protocol_with_fragments(
                [vec![character_fragment; 4], vec!["x".to_owned()]].concat(),
            ),
        };
        let (events, _receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let failure = run_generation(&over_characters, request(), None, events, cancelled)
            .await
            .expect_err("tool-argument character overflow must fail");
        assert_eq!(
            failure.error.message,
            "provider tool-call arguments exceeded the 262144-byte or 131072-character safety limit"
        );

        let byte_fragment = "😀".repeat(lorepia_domain::MAX_TOOL_ARGUMENT_DELTA_BYTES / "😀".len());
        assert_eq!(
            byte_fragment.len(),
            lorepia_domain::MAX_TOOL_ARGUMENT_DELTA_BYTES
        );
        let exact_bytes = ProtocolProvider {
            protocol_events: tool_protocol_with_fragments(vec![byte_fragment.clone(); 4]),
        };
        let (events, _receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        run_generation(&exact_bytes, request(), None, events, cancelled)
            .await
            .expect("exact cumulative tool-argument byte limit");

        let over_bytes = ProtocolProvider {
            protocol_events: tool_protocol_with_fragments(
                [vec![byte_fragment; 4], vec!["😀".to_owned()]].concat(),
            ),
        };
        let (events, _receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let failure = run_generation(&over_bytes, request(), None, events, cancelled)
            .await
            .expect_err("tool-argument byte overflow must fail");
        assert_eq!(failure.error.code, CoreErrorCode::ProviderUnavailable);
        assert!(!failure.error.recoverable);
    }

    fn completed_tool_calls(count: usize) -> Vec<ProviderEvent> {
        let mut events = Vec::with_capacity(count.saturating_mul(2));
        for index in 0..count {
            let id = ToolCallId::parse(format!("call-{index}")).expect("bounded tool call id");
            events.push(ProviderEvent::ToolCallStarted {
                id: id.clone(),
                name: ToolName::parse("lookup").expect("valid tool name"),
            });
            events.push(ProviderEvent::ToolCallCompleted { id });
        }
        events
    }

    #[tokio::test]
    async fn enforces_the_inert_tool_call_count_limit() {
        let exact = ProtocolProvider {
            protocol_events: completed_tool_calls(MAX_GENERATED_TOOL_CALLS),
        };
        let (events, _receiver) = mpsc::channel(MAX_GENERATED_TOOL_CALLS * 2 + 4);
        let (_cancel, cancelled) = watch::channel(false);
        run_generation(&exact, request(), None, events, cancelled)
            .await
            .expect("exact tool-call count");

        let overflow = ProtocolProvider {
            protocol_events: completed_tool_calls(MAX_GENERATED_TOOL_CALLS + 1),
        };
        let (events, _receiver) = mpsc::channel(MAX_GENERATED_TOOL_CALLS * 2 + 4);
        let (_cancel, cancelled) = watch::channel(false);
        let failure = run_generation(&overflow, request(), None, events, cancelled)
            .await
            .expect_err("tool-call count overflow must fail");
        assert_eq!(failure.error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(
            failure.error.message,
            "provider exceeded the 128 tool-call generation limit"
        );
    }

    #[tokio::test]
    async fn enforces_a_total_provider_event_limit_including_empty_deltas() {
        let exact = ProtocolProvider {
            protocol_events: vec![
                ProviderEvent::ReasoningDelta(String::new());
                MAX_GENERATED_PROVIDER_EVENTS
            ],
        };
        let (events, _receiver) = mpsc::channel(MAX_GENERATED_PROVIDER_EVENTS + 2);
        let (_cancel, cancelled) = watch::channel(false);
        run_generation(&exact, request(), None, events, cancelled)
            .await
            .expect("exact provider-event count");

        let overflow = ProtocolProvider {
            protocol_events: vec![
                ProviderEvent::ReasoningDelta(String::new());
                MAX_GENERATED_PROVIDER_EVENTS + 1
            ],
        };
        let (events, _receiver) = mpsc::channel(MAX_GENERATED_PROVIDER_EVENTS + 2);
        let (_cancel, cancelled) = watch::channel(false);
        let failure = run_generation(&overflow, request(), None, events, cancelled)
            .await
            .expect_err("provider-event overflow must fail");
        assert_eq!(failure.error.code, CoreErrorCode::ProviderUnavailable);
        assert_eq!(
            failure.error.message,
            "provider exceeded the 8192-event generation limit"
        );
        assert!(!failure.error.recoverable);
        assert_eq!(
            failure.last_sequence,
            u64::try_from(MAX_GENERATED_PROVIDER_EVENTS + 1).expect("bounded sequence")
        );
    }

    #[tokio::test]
    async fn rejects_arguments_before_start_and_incomplete_calls() {
        let invalid_cases = [
            vec![ProviderEvent::ToolCallArgumentsDelta {
                id: tool_id(),
                delta: ToolCallArgumentsDelta::parse("{}").expect("valid arguments"),
            }],
            vec![ProviderEvent::ToolCallStarted {
                id: tool_id(),
                name: ToolName::parse("lookup").expect("valid tool name"),
            }],
        ];

        for protocol_events in invalid_cases {
            let provider = ProtocolProvider { protocol_events };
            let (events, _receiver) = mpsc::channel(16);
            let (_cancel, cancelled) = watch::channel(false);
            let failure = run_generation(&provider, request(), None, events, cancelled)
                .await
                .expect_err("invalid tool protocol must fail");
            assert_eq!(failure.error.code, CoreErrorCode::ProviderUnavailable);
            assert!(!failure.error.recoverable);
        }
    }

    #[tokio::test]
    async fn blocks_exact_credential_across_every_utf8_text_delta_split() {
        let credential = "sk-비밀-🔑-tail";
        for (split, _) in credential.char_indices().skip(1) {
            let (result, events) = run_scripted(
                credential,
                vec![
                    ProviderEvent::TextDelta(format!("visible:{}", &credential[..split])),
                    ProviderEvent::TextDelta(format!("{}:hidden", &credential[split..])),
                ],
                Ok(GenerationUsage::default()),
                false,
            )
            .await;
            let failure = assert_reflection_failure(result, &events, credential);
            assert_eq!(failure.partial_text, "visible:");
            let forwarded = events
                .iter()
                .filter_map(|event| match &event.kind {
                    ChatEventKind::TextDelta(delta) => Some(delta.as_str()),
                    _ => None,
                })
                .collect::<String>();
            assert_eq!(forwarded, "visible:");
        }
    }

    #[tokio::test]
    async fn blocks_exact_credential_across_every_utf8_reasoning_delta_split() {
        let credential = "sk-비밀-🔑-tail";
        for (split, _) in credential.char_indices().skip(1) {
            let (result, events) = run_scripted(
                credential,
                vec![
                    ProviderEvent::ReasoningDelta(format!(
                        "visible-reasoning:{}",
                        &credential[..split]
                    )),
                    ProviderEvent::ReasoningDelta(credential[split..].to_owned()),
                ],
                Ok(GenerationUsage::default()),
                false,
            )
            .await;
            let failure = assert_reflection_failure(result, &events, credential);
            assert!(failure.partial_text.is_empty());
            let forwarded = events
                .iter()
                .filter_map(|event| match &event.kind {
                    ChatEventKind::ReasoningDelta(delta) => Some(delta.as_str()),
                    _ => None,
                })
                .collect::<String>();
            assert_eq!(forwarded, "visible-reasoning:");
        }
    }

    #[tokio::test]
    async fn blocks_exact_credential_across_every_utf8_tool_argument_split() {
        let credential = "sk-비밀-🔑-tail";
        for (split, _) in credential.char_indices().skip(1) {
            let protocol_events = vec![
                ProviderEvent::ToolCallStarted {
                    id: tool_id(),
                    name: ToolName::parse("lookup").expect("valid tool name"),
                },
                ProviderEvent::ToolCallArgumentsDelta {
                    id: tool_id(),
                    delta: ToolCallArgumentsDelta::parse(format!(
                        "{{\"query\":\"{}",
                        &credential[..split]
                    ))
                    .expect("valid first fragment"),
                },
                ProviderEvent::ToolCallArgumentsDelta {
                    id: tool_id(),
                    delta: ToolCallArgumentsDelta::parse(format!("{}\"}}", &credential[split..]))
                        .expect("valid second fragment"),
                },
                ProviderEvent::ToolCallCompleted { id: tool_id() },
            ];
            let (result, events) = run_scripted(
                credential,
                protocol_events,
                Ok(GenerationUsage::default()),
                false,
            )
            .await;
            let failure = assert_reflection_failure(result, &events, credential);
            assert!(failure.partial_text.is_empty());
            let forwarded = events
                .iter()
                .filter_map(|event| match &event.kind {
                    ChatEventKind::ToolCallArgumentsDelta { delta, .. } => Some(delta.as_str()),
                    _ => None,
                })
                .collect::<String>();
            assert_eq!(forwarded, "{\"query\":\"");
        }
    }

    #[tokio::test]
    async fn resegments_large_safe_tool_release_after_withholding_a_long_prefix() {
        let credential = "k".repeat(MAX_BORROWED_CREDENTIAL_BYTES);
        let withheld_prefix = credential[..credential.len() - 1].to_owned();
        let following_fragment =
            "😀".repeat(lorepia_domain::MAX_TOOL_ARGUMENT_DELTA_BYTES / "😀".len());
        let expected = format!("{withheld_prefix}{following_fragment}");
        let protocol_events = vec![
            ProviderEvent::ToolCallStarted {
                id: tool_id(),
                name: ToolName::parse("lookup").expect("tool name"),
            },
            ProviderEvent::ToolCallArgumentsDelta {
                id: tool_id(),
                delta: ToolCallArgumentsDelta::parse(withheld_prefix)
                    .expect("withheld tool prefix"),
            },
            ProviderEvent::ToolCallArgumentsDelta {
                id: tool_id(),
                delta: ToolCallArgumentsDelta::parse(following_fragment)
                    .expect("maximum byte fragment"),
            },
            ProviderEvent::ToolCallCompleted { id: tool_id() },
        ];
        let (result, events) = run_scripted(
            &credential,
            protocol_events,
            Ok(GenerationUsage::default()),
            false,
        )
        .await;
        result.expect("safe combined release must be resegmented");

        let mut forwarded = String::new();
        for event in events {
            if let ChatEventKind::ToolCallArgumentsDelta { delta, .. } = event.kind {
                assert!(delta.as_str().len() <= lorepia_domain::MAX_TOOL_ARGUMENT_DELTA_BYTES);
                assert!(
                    delta.as_str().chars().count() <= lorepia_domain::MAX_TOOL_ARGUMENT_DELTA_CHARS
                );
                forwarded.push_str(delta.as_str());
            }
        }
        assert_eq!(forwarded, expected);
        assert!(!forwarded.contains(&credential));
    }

    #[tokio::test]
    async fn blocks_exact_credential_in_complete_tool_ids_and_names() {
        let credential = "sk-tool-reflection";
        let cases = [
            ProviderEvent::ToolCallStarted {
                id: ToolCallId::parse(format!("call-{credential}")).expect("valid reflected id"),
                name: ToolName::parse("lookup").expect("valid tool name"),
            },
            ProviderEvent::ToolCallStarted {
                id: tool_id(),
                name: ToolName::parse(format!("lookup-{credential}"))
                    .expect("valid reflected name"),
            },
        ];

        for event in cases {
            let (result, events) = run_scripted(
                credential,
                vec![event],
                Ok(GenerationUsage::default()),
                false,
            )
            .await;
            let failure = assert_reflection_failure(result, &events, credential);
            assert!(failure.partial_text.is_empty());
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event.kind, ChatEventKind::ToolCallStarted { .. }))
            );
        }
    }

    #[tokio::test]
    async fn blocks_exact_credential_in_every_opaque_reasoning_family() {
        let credential = "sk-opaque-비밀";
        let openai_item = OpenAiResponsesReasoningItem::from_value(&serde_json::json!({
            "id": "rs_safe",
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": credential}],
            "encrypted_content": "encrypted-safe",
            "status": "completed"
        }))
        .expect("valid OpenAI reasoning item");
        let openrouter_detail = OpenRouterReasoningDetail::from_value(&serde_json::json!({
            "type": "reasoning.text",
            "id": null,
            "format": "openrouter-v1",
            "text": credential,
            "signature": "signature-safe"
        }))
        .expect("valid OpenRouter detail");
        let openrouter_topology =
            OpenRouterReasoningTopology::new(None, Some(vec![openrouter_detail]))
                .expect("valid OpenRouter topology");
        let anthropic_input = AnthropicToolInput::from_value(&serde_json::json!({
            "nested": {"provider_token": credential}
        }))
        .expect("valid Anthropic tool input");
        let anthropic_topology = AnthropicContentBlockTopology::new(vec![
            AnthropicContentBlock::Thinking {
                thinking: AnthropicBlockText::parse("thinking-safe").expect("thinking"),
                signature: OpaqueReasoningData::parse("signature-safe").expect("signature"),
            },
            AnthropicContentBlock::ToolUse {
                id: ToolCallId::parse("call-safe").expect("tool id"),
                name: ToolName::parse("lookup").expect("tool name"),
                input: anthropic_input,
            },
        ])
        .expect("valid Anthropic topology");
        let states = vec![
            (
                ApiFamily::OpenAiResponses,
                OpaqueReasoningState::OpenAiResponses { item: openai_item },
            ),
            (
                ApiFamily::GeminiGenerateContent,
                OpaqueReasoningState::GeminiThoughtSignature {
                    part_index: 0,
                    signature: OpaqueReasoningData::parse(credential).expect("Gemini signature"),
                },
            ),
            (
                ApiFamily::OpenAiChatCompletions,
                OpaqueReasoningState::OpenRouterReasoning {
                    topology: openrouter_topology,
                },
            ),
            (
                ApiFamily::AnthropicMessages,
                OpaqueReasoningState::AnthropicMessages {
                    content_blocks: anthropic_topology,
                },
            ),
        ];

        for (provider_family, state) in states {
            let (result, events) = run_scripted_for_family(
                credential,
                vec![ProviderEvent::OpaqueReasoningState(state)],
                Ok(GenerationUsage::default()),
                true,
                Some(provider_family),
            )
            .await;
            let failure = assert_reflection_failure(result, &events, credential);
            assert!(failure.partial_text.is_empty());
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event.kind, ChatEventKind::UsageUpdated(_)))
            );
        }
    }

    #[tokio::test]
    async fn scans_complete_openrouter_topology_and_preserves_an_empty_details_marker() {
        let credential = "sk-openrouter-topology";
        let plaintext = OpenRouterReasoningTopology::new(
            Some(format!("safe prefix {credential} safe suffix")),
            Some(Vec::new()),
        )
        .expect("OpenRouter plaintext topology");

        let mut keyed_detail = serde_json::json!({
            "type": "reasoning.text",
            "id": null,
            "format": "openrouter-v1",
            "text": "reasoning-safe",
            "signature": null
        })
        .as_object()
        .expect("detail object")
        .clone();
        keyed_detail.insert(
            format!("provider-{credential}"),
            serde_json::json!({"nested": "safe"}),
        );
        let keyed = OpenRouterReasoningTopology::new(
            None,
            Some(vec![
                OpenRouterReasoningDetail::from_value(&serde_json::Value::Object(keyed_detail))
                    .expect("OpenRouter detail with reflected key"),
            ]),
        )
        .expect("OpenRouter keyed topology");

        for topology in [plaintext, keyed] {
            let (result, events) = run_scripted_for_family(
                credential,
                vec![ProviderEvent::OpaqueReasoningState(
                    OpaqueReasoningState::OpenRouterReasoning { topology },
                )],
                Ok(GenerationUsage::default()),
                true,
                Some(ApiFamily::OpenAiChatCompletions),
            )
            .await;
            let failure = assert_reflection_failure(result, &events, credential);
            assert!(failure.partial_text.is_empty());
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event.kind, ChatEventKind::UsageUpdated(_)))
            );
        }

        let empty_marker = OpaqueReasoningState::OpenRouterReasoning {
            topology: OpenRouterReasoningTopology::new(None, Some(Vec::new()))
                .expect("observed empty details marker"),
        };
        let (result, events) = run_scripted_for_family(
            credential,
            vec![ProviderEvent::OpaqueReasoningState(empty_marker.clone())],
            Ok(GenerationUsage::default()),
            true,
            Some(ApiFamily::OpenAiChatCompletions),
        )
        .await;
        let outcome = result.expect("safe empty marker must be retained");
        assert_eq!(outcome.opaque_reasoning_state, vec![empty_marker]);
        assert!(
            events
                .iter()
                .any(|event| matches!(event.kind, ChatEventKind::UsageUpdated(_)))
        );
    }

    #[tokio::test]
    async fn rejects_mismatched_mixed_and_ollama_opaque_state_before_outcome() {
        let credential = "credential-not-in-state";
        let gemini_state = || OpaqueReasoningState::GeminiThoughtSignature {
            part_index: 0,
            signature: OpaqueReasoningData::parse("signature-safe").expect("signature"),
        };
        let openrouter_state = || OpaqueReasoningState::OpenRouterReasoning {
            topology: OpenRouterReasoningTopology::new(
                Some("reasoning-safe".to_owned()),
                Some(vec![
                    OpenRouterReasoningDetail::from_value(&serde_json::json!({
                        "type": "reasoning.text",
                        "id": null,
                        "format": "openrouter-v1",
                        "text": "reasoning-safe",
                        "signature": null
                    }))
                    .expect("OpenRouter detail"),
                ]),
            )
            .expect("OpenRouter topology"),
        };
        let cases = [
            (
                ApiFamily::OpenAiResponses,
                vec![gemini_state()],
                "mismatched family",
            ),
            (
                ApiFamily::GeminiGenerateContent,
                vec![gemini_state(), openrouter_state()],
                "mixed families",
            ),
            (
                ApiFamily::OllamaNative,
                vec![gemini_state()],
                "Ollama opaque state",
            ),
        ];

        for (provider_family, states, label) in cases {
            let protocol_events = states
                .into_iter()
                .map(ProviderEvent::OpaqueReasoningState)
                .collect();
            let (result, events) = run_scripted_for_family(
                credential,
                protocol_events,
                Ok(GenerationUsage::default()),
                true,
                Some(provider_family),
            )
            .await;
            let failure = result.expect_err(label);
            assert_eq!(failure.error.code, CoreErrorCode::ProviderUnavailable);
            assert_eq!(
                failure.error.message,
                "provider returned opaque reasoning state for a different API family"
            );
            assert!(failure.partial_text.is_empty());
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event.kind, ChatEventKind::UsageUpdated(_)))
            );
        }
    }

    #[tokio::test]
    async fn blocks_escaped_credential_in_usage_summary_keys_and_values() {
        let credential = "sk-\"quoted\\비밀";
        let value_summary = BoundedJson::from_value(&serde_json::json!({
            "nested": {"provider_value": credential}
        }))
        .expect("bounded reflected value");
        let mut reflected_key = serde_json::Map::new();
        reflected_key.insert(
            format!("provider-{credential}"),
            serde_json::Value::from(1_u64),
        );
        let key_summary = BoundedJson::from_value(&serde_json::Value::Object(reflected_key))
            .expect("bounded reflected key");
        let withheld = &credential[..3];

        for summary in [value_summary, key_summary] {
            let usage = GenerationUsage {
                provider_raw_summary: Some(summary),
                ..GenerationUsage::default()
            };
            let (result, events) = run_scripted(
                credential,
                vec![ProviderEvent::TextDelta(format!("visible:{withheld}"))],
                Ok(usage),
                false,
            )
            .await;
            let failure = assert_reflection_failure(result, &events, credential);
            assert_eq!(failure.partial_text, "visible:");
            assert!(!failure.partial_text.contains(withheld));
            assert!(
                events
                    .iter()
                    .all(|event| !chat_event_contains_exact(event, withheld))
            );
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event.kind, ChatEventKind::UsageUpdated(_)))
            );
        }
    }

    #[tokio::test]
    async fn sanitizes_reflected_provider_error_and_drops_pending_prefix() {
        let credential = "sk-error-비밀";
        let prefix_end = credential
            .char_indices()
            .nth(5)
            .map_or(credential.len() - 1, |(index, _)| index);
        let withheld = &credential[..prefix_end];
        let error = CoreError::new(
            CoreErrorCode::ProviderAuthFailed,
            format!("upstream echoed {credential}"),
            false,
        );
        let (result, events) = run_scripted(
            credential,
            vec![ProviderEvent::TextDelta(format!("visible:{withheld}"))],
            Err(error),
            false,
        )
        .await;
        let failure = assert_reflection_failure(result, &events, credential);
        assert_eq!(failure.partial_text, "visible:");
        assert!(!failure.partial_text.contains(withheld));
        assert!(
            events
                .iter()
                .all(|event| !chat_event_contains_exact(event, withheld))
        );
    }

    #[tokio::test]
    async fn provider_error_drops_completed_tool_argument_suffix_and_completion() {
        let credential = "sk-tool-error-비밀";
        let prefix_end = credential
            .char_indices()
            .nth(7)
            .map_or(credential.len() - 1, |(index, _)| index);
        let withheld = &credential[..prefix_end];
        let protocol_events = vec![
            ProviderEvent::ToolCallStarted {
                id: tool_id(),
                name: ToolName::parse("lookup").expect("tool name"),
            },
            ProviderEvent::ToolCallArgumentsDelta {
                id: tool_id(),
                delta: ToolCallArgumentsDelta::parse(format!("{{\"q\":\"{withheld}"))
                    .expect("tool arguments"),
            },
            ProviderEvent::ToolCallCompleted { id: tool_id() },
        ];
        let provider_error = CoreError::new(
            CoreErrorCode::NetworkUnavailable,
            "upstream disconnected",
            true,
        );
        let (result, events) = run_scripted(
            credential,
            protocol_events,
            Err(provider_error.clone()),
            false,
        )
        .await;
        let failure =
            result.expect_err("provider failure must discard completed pending tool data");
        assert_eq!(failure.error, provider_error);
        assert!(failure.partial_text.is_empty());
        let forwarded = events
            .iter()
            .filter_map(|event| match &event.kind {
                ChatEventKind::ToolCallArgumentsDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(forwarded, "{\"q\":\"");
        assert!(!forwarded.contains(withheld));
        assert!(
            events
                .iter()
                .all(|event| !matches!(event.kind, ChatEventKind::ToolCallCompleted { .. }))
        );
    }

    #[tokio::test]
    async fn flushes_a_safe_utf8_credential_prefix_only_after_success() {
        let credential = "sk-success-비밀-🔑";
        let prefix_end = credential
            .char_indices()
            .nth(7)
            .map_or(credential.len() - 1, |(index, _)| index);
        let safe_output = format!("visible:{}", &credential[..prefix_end]);
        let (result, events) = run_scripted(
            credential,
            vec![ProviderEvent::TextDelta(safe_output.clone())],
            Ok(GenerationUsage::default()),
            false,
        )
        .await;
        let outcome = result.expect("proper prefix is safe at a successful stream boundary");
        assert_eq!(outcome.text, safe_output);
        assert!(!outcome.text.contains(credential));
        let forwarded = events
            .iter()
            .filter_map(|event| match &event.kind {
                ChatEventKind::TextDelta(delta) => Some(delta.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(forwarded, outcome.text);
    }

    struct PrefixThenPendingProvider {
        chunk: String,
    }

    #[async_trait]
    impl Provider for PrefixThenPendingProvider {
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
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            sink.send(ProviderEvent::TextDelta(self.chunk.clone()))
                .await
                .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            std::future::pending().await
        }
    }

    struct ProtocolThenPendingProvider {
        protocol_events: Vec<ProviderEvent>,
    }

    #[async_trait]
    impl Provider for ProtocolThenPendingProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: true,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            for event in &self.protocol_events {
                sink.send(event.clone())
                    .await
                    .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            }
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn cancellation_drops_a_withheld_credential_prefix() {
        let credential = "sk-cancel-비밀".to_owned();
        let prefix_end = credential
            .char_indices()
            .nth(6)
            .map_or(credential.len() - 1, |(index, _)| index);
        let withheld = credential[..prefix_end].to_owned();
        let provider = PrefixThenPendingProvider {
            chunk: format!("visible:{withheld}"),
        };
        let (events, mut receiver) = mpsc::channel(16);
        let (cancel, cancelled) = watch::channel(false);
        let credential_for_generation = credential.clone();
        let generation = tokio::spawn(async move {
            run_generation(
                &provider,
                request(),
                Some(&credential_for_generation),
                events,
                cancelled,
            )
            .await
        });

        loop {
            let event = receiver.recv().await.expect("generation event");
            if matches!(&event.kind, ChatEventKind::TextDelta(delta) if delta == "visible:") {
                break;
            }
        }
        cancel.send(true).expect("cancel generation");
        let failure = time::timeout(Duration::from_secs(1), generation)
            .await
            .expect("cancellation timeout")
            .expect("generation task")
            .expect_err("generation must cancel");
        assert_eq!(failure.error.code, CoreErrorCode::Cancelled);
        assert_eq!(failure.partial_text, "visible:");
        assert!(!failure.partial_text.contains(&withheld));
        while let Ok(event) = receiver.try_recv() {
            assert!(!chat_event_contains_exact(&event, &withheld));
        }
    }

    #[tokio::test]
    async fn cancellation_drops_completed_tool_suffix_and_completion() {
        let credential = "sk-tool-cancel-비밀".to_owned();
        let prefix_end = credential
            .char_indices()
            .nth(8)
            .map_or(credential.len() - 1, |(index, _)| index);
        let withheld = credential[..prefix_end].to_owned();
        let provider = ProtocolThenPendingProvider {
            protocol_events: vec![
                ProviderEvent::ToolCallStarted {
                    id: tool_id(),
                    name: ToolName::parse("lookup").expect("tool name"),
                },
                ProviderEvent::ToolCallArgumentsDelta {
                    id: tool_id(),
                    delta: ToolCallArgumentsDelta::parse(format!("{{\"q\":\"{withheld}"))
                        .expect("tool arguments"),
                },
                ProviderEvent::ToolCallCompleted { id: tool_id() },
                ProviderEvent::TextDelta("after-completion".to_owned()),
            ],
        };
        let (events, mut receiver) = mpsc::channel(32);
        let (cancel, cancelled) = watch::channel(false);
        let credential_for_generation = credential.clone();
        let generation = tokio::spawn(async move {
            run_generation(
                &provider,
                request(),
                Some(&credential_for_generation),
                events,
                cancelled,
            )
            .await
        });

        let mut observed = Vec::new();
        loop {
            let event = receiver.recv().await.expect("generation event");
            let after_completion = matches!(&event.kind, ChatEventKind::TextDelta(delta) if delta == "after-completion");
            observed.push(event);
            if after_completion {
                break;
            }
        }
        cancel.send(true).expect("cancel generation");
        let failure = time::timeout(Duration::from_secs(1), generation)
            .await
            .expect("cancellation timeout")
            .expect("generation task")
            .expect_err("generation must cancel");
        observed.extend(std::iter::from_fn(|| receiver.try_recv().ok()));

        assert_eq!(failure.error.code, CoreErrorCode::Cancelled);
        assert!(failure.partial_text.contains("after-completion"));
        assert!(!failure.partial_text.contains(&withheld));
        assert!(
            observed
                .iter()
                .all(|event| !chat_event_contains_exact(event, &withheld))
        );
        assert!(
            observed
                .iter()
                .all(|event| !matches!(event.kind, ChatEventKind::ToolCallCompleted { .. }))
        );
    }

    struct GatedSuccessProvider {
        chunk: String,
        finish: Arc<Notify>,
    }

    #[async_trait]
    impl Provider for GatedSuccessProvider {
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
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            sink.send(ProviderEvent::TextDelta(self.chunk.clone()))
                .await
                .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            self.finish.notified().await;
            Ok(GenerationUsage::default())
        }
    }

    #[tokio::test]
    async fn failed_final_event_send_does_not_add_withheld_prefix_to_partial_text() {
        let credential = "sk-final-send-비밀".to_owned();
        let prefix_end = credential
            .char_indices()
            .nth(7)
            .map_or(credential.len() - 1, |(index, _)| index);
        let withheld = credential[..prefix_end].to_owned();
        let finish = Arc::new(Notify::new());
        let provider = GatedSuccessProvider {
            chunk: format!("visible:{withheld}"),
            finish: Arc::clone(&finish),
        };
        let (events, mut receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let credential_for_generation = credential.clone();
        let generation = tokio::spawn(async move {
            run_generation(
                &provider,
                request(),
                Some(&credential_for_generation),
                events,
                cancelled,
            )
            .await
        });

        loop {
            let event = receiver.recv().await.expect("generation event");
            if matches!(&event.kind, ChatEventKind::TextDelta(delta) if delta == "visible:") {
                break;
            }
        }
        drop(receiver);
        finish.notify_one();

        let failure = time::timeout(Duration::from_secs(1), generation)
            .await
            .expect("generation timeout")
            .expect("generation task")
            .expect_err("closed final event receiver must fail");
        assert_eq!(failure.error.code, CoreErrorCode::Internal);
        assert_eq!(failure.partial_text, "visible:");
        assert!(!failure.partial_text.contains(&withheld));
    }

    #[tokio::test]
    async fn rejects_oversized_credentials_before_start_or_dispatch() {
        let credential = "x".repeat(MAX_BORROWED_CREDENTIAL_BYTES + 1);
        let (events, mut receiver) = mpsc::channel(16);
        let (_cancel, cancelled) = watch::channel(false);
        let failure = run_generation(
            &StaticProvider::new("must not run"),
            request(),
            Some(&credential),
            events,
            cancelled,
        )
        .await
        .expect_err("oversized credential must be rejected before dispatch");
        assert_eq!(failure.error.code, CoreErrorCode::InvalidInput);
        assert_eq!(failure.error.message, CREDENTIAL_LIMIT_ERROR_MESSAGE);
        assert!(failure.partial_text.is_empty());
        assert!(receiver.try_recv().is_err());
    }
}
