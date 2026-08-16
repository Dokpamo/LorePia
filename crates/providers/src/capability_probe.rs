use std::{collections::HashSet, future::pending, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lorepia_domain::{
    ApiFamily, CapabilityKey, CapabilityObservation, CapabilityValue, Confidence, ConversationId,
    CoreError, CoreErrorCode, CoreResult, EvidenceId, GenerationId, GenerationRequest,
    GenerationUsage, Message, ModelRouteId, ObservationId, ObservationSource, SupportStatus,
};
use tokio::sync::{Mutex, mpsc, watch};

use crate::{
    Provider, ProviderEvent,
    parameter_mapping::ProviderRequestPlan,
    request_plan::{
        CAPABILITY_PROBE_TOOL_NAME, CapabilityProbePlanKind, capability_probe_request_plan,
    },
};

const MAX_PROBE_TOTAL_TOKENS: u64 = 4_096;
const MAX_PROBE_OUTPUT_TOKENS: u64 = 1_024;
const MAX_PROBE_COST_MICRO_USD: u64 = 100_000;
const MAX_PROBE_DURATION: Duration = Duration::from_mins(1);
const PROBE_CALL_LIMIT: u32 = 1;
const DEFAULT_OBSERVATION_FRESHNESS_SECONDS: i64 = 7 * 24 * 60 * 60;
const CACHE_OBSERVATION_FRESHNESS_SECONDS: i64 = 24 * 60 * 60;
const MAX_CONSUMED_CONSENTS: usize = 4_096;
const MAX_MERGE_OBSERVATIONS: usize = 256;
const PROBE_EVENT_CHANNEL_CAPACITY: usize = 8;
const MAX_PROBE_EVENT_COUNT: u32 = 256;
const MAX_PROBE_EVENT_BYTES: usize = 64 * 1024;
const MAX_PROBE_MODEL_ID_BYTES: usize = 1_024;
const BASIC_PROBE_INPUT_TOKEN_RESERVATION: u64 = 64;
const STRUCTURED_PROBE_INPUT_TOKEN_RESERVATION: u64 = 192;
const TOOL_PROBE_INPUT_TOKEN_RESERVATION: u64 = 256;
const PROBE_OUTPUT_TOKEN_LIMIT: u64 = 32;
const SYNTHETIC_PROBE_PROMPT: &str = "Reply only with probe-ok.";
const STRUCTURED_OUTPUT_PROBE_PROMPT: &str =
    "Return the JSON object whose probe property is exactly ok.";
const TOOL_CALL_PROBE_PROMPT: &str =
    "Call lorepia_probe with its probe argument set to ok. Do not answer with text.";

/// A capability that can be tested with a small, synthetic provider request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityProbeKind {
    Streaming,
    Reasoning,
    StructuredOutput,
    ToolCalling,
    PromptCaching,
}

impl From<CapabilityProbeKind> for CapabilityKey {
    fn from(value: CapabilityProbeKind) -> Self {
        match value {
            CapabilityProbeKind::Streaming => Self::Streaming,
            CapabilityProbeKind::Reasoning => Self::Reasoning,
            CapabilityProbeKind::StructuredOutput => Self::StructuredOutput,
            CapabilityProbeKind::ToolCalling => Self::ToolCalling,
            CapabilityProbeKind::PromptCaching => Self::PromptCaching,
        }
    }
}

/// The strict cost and resource envelope attached to one explicit consent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeBudget {
    total_token_limit: u64,
    output_token_limit: u64,
    cost_limit_micro_usd: u64,
    time_limit: Duration,
    call_limit: u32,
}

impl ProbeBudget {
    pub fn new(
        max_total_tokens: u64,
        max_output_tokens: u64,
        max_cost_micro_usd: u64,
        max_duration: Duration,
        max_calls: u32,
    ) -> CoreResult<Self> {
        if max_total_tokens == 0 || max_total_tokens > MAX_PROBE_TOTAL_TOKENS {
            return Err(CoreError::invalid(format!(
                "probe total-token cap must be between 1 and {MAX_PROBE_TOTAL_TOKENS}"
            )));
        }
        if max_output_tokens == 0
            || max_output_tokens > max_total_tokens
            || max_output_tokens > MAX_PROBE_OUTPUT_TOKENS
        {
            return Err(CoreError::invalid(format!(
                "probe output-token cap must be between 1 and {}",
                MAX_PROBE_OUTPUT_TOKENS.min(max_total_tokens)
            )));
        }
        if max_cost_micro_usd > MAX_PROBE_COST_MICRO_USD {
            return Err(CoreError::invalid(format!(
                "probe cost cap exceeds {MAX_PROBE_COST_MICRO_USD} micro-USD"
            )));
        }
        if max_duration.is_zero() || max_duration > MAX_PROBE_DURATION {
            return Err(CoreError::invalid(format!(
                "probe time cap must be between 1ns and {}s",
                MAX_PROBE_DURATION.as_secs()
            )));
        }
        if max_calls != PROBE_CALL_LIMIT {
            return Err(CoreError::invalid(
                "each capability probe consent authorizes exactly one provider call",
            ));
        }
        Ok(Self {
            total_token_limit: max_total_tokens,
            output_token_limit: max_output_tokens,
            cost_limit_micro_usd: max_cost_micro_usd,
            time_limit: max_duration,
            call_limit: max_calls,
        })
    }

    pub fn max_total_tokens(self) -> u64 {
        self.total_token_limit
    }

    pub fn max_output_tokens(self) -> u64 {
        self.output_token_limit
    }

    pub fn max_cost_micro_usd(self) -> u64 {
        self.cost_limit_micro_usd
    }

    pub fn max_duration(self) -> Duration {
        self.time_limit
    }

    pub fn max_calls(self) -> u32 {
        self.call_limit
    }
}

/// One-shot user consent for one route, one probe, and one fixed budget.
///
/// The value intentionally does not implement `Clone`; the engine also tracks
/// consent IDs to prevent replay after reconstruction at a higher layer.
#[derive(Debug)]
pub struct ProbeConsent {
    id: String,
    model_route_id: ModelRouteId,
    probe: CapabilityProbeKind,
    budget: ProbeBudget,
}

impl ProbeConsent {
    pub fn new(
        id: impl Into<String>,
        model_route_id: ModelRouteId,
        probe: CapabilityProbeKind,
        budget: ProbeBudget,
    ) -> CoreResult<Self> {
        let id = id.into();
        if !is_canonical_uuid(&id) {
            return Err(CoreError::invalid(
                "probe consent ID must be a canonical, non-secret UUID",
            ));
        }
        Ok(Self {
            id,
            model_route_id,
            probe,
            budget,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn model_route_id(&self) -> &ModelRouteId {
        &self.model_route_id
    }

    pub fn probe(&self) -> CapabilityProbeKind {
        self.probe
    }

    pub fn budget(&self) -> ProbeBudget {
        self.budget
    }
}

/// Bounded request visible to an adapter. It contains no credential or user content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterProbeRequest {
    model_route_id: ModelRouteId,
    adapter_family: ApiFamily,
    probe: CapabilityProbeKind,
    budget: ProbeBudget,
}

impl AdapterProbeRequest {
    pub fn model_route_id(&self) -> &ModelRouteId {
        &self.model_route_id
    }

    pub fn adapter_family(&self) -> ApiFamily {
        self.adapter_family
    }

    pub fn probe(&self) -> CapabilityProbeKind {
        self.probe
    }

    pub fn budget(&self) -> ProbeBudget {
        self.budget
    }
}

/// Structured proof returned by an adapter, never a raw provider body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeEvidence {
    StreamingCompleted { event_count: u32 },
    ReasoningObserved,
    StructuredOutputValidated,
    ToolCallRoundTrip,
    PromptCacheHit { cache_read_tokens: u64 },
    ExplicitlyUnsupported,
}

/// Measured resource use for the single authorized provider call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeUsage {
    input_tokens: u64,
    output_tokens: u64,
    cost_micro_usd: u64,
    calls: u32,
}

impl ProbeUsage {
    pub fn new(input_tokens: u64, output_tokens: u64, cost_micro_usd: u64, calls: u32) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cost_micro_usd,
            calls,
        }
    }

    pub fn input_tokens(self) -> u64 {
        self.input_tokens
    }

    pub fn output_tokens(self) -> u64 {
        self.output_tokens
    }

    pub fn cost_micro_usd(self) -> u64 {
        self.cost_micro_usd
    }

    pub fn calls(self) -> u32 {
        self.calls
    }
}

/// A successful protocol-level response from an adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterProbeResult {
    supported: bool,
    evidence: ProbeEvidence,
    usage: ProbeUsage,
}

impl AdapterProbeResult {
    pub fn new(supported: bool, evidence: ProbeEvidence, usage: ProbeUsage) -> Self {
        Self {
            supported,
            evidence,
            usage,
        }
    }

    pub fn supported(&self) -> bool {
        self.supported
    }

    pub fn evidence(&self) -> &ProbeEvidence {
        &self.evidence
    }

    pub fn usage(&self) -> ProbeUsage {
        self.usage
    }
}

/// Redacted adapter error classes. There is intentionally no provider-supplied text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeAdapterError {
    Authentication,
    RateLimited,
    Unavailable,
    InvalidRequest,
    ProtocolViolation,
    Interrupted,
}

/// Adapter boundary used by the probe engine.
///
/// Implementations may hold a request-scoped credential internally, but the
/// engine request and result types cannot contain or persist one.
#[async_trait]
pub trait CapabilityProbeAdapter: Send + Sync {
    fn family(&self) -> ApiFamily;

    /// Probe kinds for which this adapter can produce exact request/response
    /// evidence. This describes the compiled adapter, not the remote model.
    fn implemented_probes(&self) -> &'static [CapabilityProbeKind] {
        const ALL: &[CapabilityProbeKind] = &[
            CapabilityProbeKind::Streaming,
            CapabilityProbeKind::Reasoning,
            CapabilityProbeKind::StructuredOutput,
            CapabilityProbeKind::ToolCalling,
            CapabilityProbeKind::PromptCaching,
        ];
        ALL
    }

    async fn execute_probe(
        &self,
        request: AdapterProbeRequest,
        cancelled: watch::Receiver<bool>,
    ) -> Result<AdapterProbeResult, ProbeAdapterError>;
}

/// Executes exact, single-call probes through a provider built by
/// [`crate::AdapterRegistry`].
///
/// The credential is borrowed for the lifetime of this request-scoped adapter;
/// it is never copied into a request/result, serialized, or included in an
/// error. The adapter accepts only synthetic LorePia-owned prompt content.
///
/// Structured-output and tool probes use only fixed, Rust-authored
/// family-specific request plans. Success requires a schema-valid response or
/// one exact completed inert tool-call event; prompt-only JSON or prose never
/// counts as evidence.
pub struct ProviderCapabilityProbeAdapter<'credential> {
    family: ApiFamily,
    model_route_id: ModelRouteId,
    model_id: String,
    provider: Arc<dyn Provider>,
    credential: Option<&'credential str>,
    probe: CapabilityProbeKind,
    call_cost_upper_bound_micro_usd: u64,
}

impl<'credential> ProviderCapabilityProbeAdapter<'credential> {
    /// Creates one request-scoped adapter for one exact probe.
    ///
    /// `call_cost_upper_bound_micro_usd` must be a conservative provider/model
    /// price ceiling supplied by the caller. It is charged against the consent
    /// budget even if the eventual invoice is lower. Pass zero only for a
    /// provider call known to be free.
    pub fn new(
        family: ApiFamily,
        model_route_id: ModelRouteId,
        model_id: impl Into<String>,
        provider: Arc<dyn Provider>,
        credential: Option<&'credential str>,
        probe: CapabilityProbeKind,
        call_cost_upper_bound_micro_usd: u64,
    ) -> CoreResult<Self> {
        let model_id = model_id.into();
        if model_id.trim().is_empty()
            || model_id.len() > MAX_PROBE_MODEL_ID_BYTES
            || model_id.chars().any(char::is_control)
        {
            return Err(CoreError::invalid(
                "capability probe model ID is empty, oversized, or contains control characters",
            ));
        }
        if call_cost_upper_bound_micro_usd > MAX_PROBE_COST_MICRO_USD {
            return Err(CoreError::invalid(
                "capability probe call-cost ceiling exceeds the global maximum",
            ));
        }

        let capabilities = provider.capabilities();
        match probe {
            CapabilityProbeKind::Streaming if !capabilities.streaming => {
                return Err(probe_not_implemented(
                    "the compiled provider does not expose streaming events",
                ));
            }
            CapabilityProbeKind::Reasoning if !capabilities.reasoning => {
                return Err(probe_not_implemented(
                    "the compiled provider does not expose reasoning evidence",
                ));
            }
            CapabilityProbeKind::PromptCaching if family == ApiFamily::OllamaNative => {
                return Err(probe_not_implemented(
                    "the compiled Ollama provider does not expose cache-read usage",
                ));
            }
            _ => {}
        }

        Ok(Self {
            family,
            model_route_id,
            model_id,
            provider,
            credential: credential.filter(|value| !value.is_empty()),
            probe,
            call_cost_upper_bound_micro_usd,
        })
    }

    pub fn probe(&self) -> CapabilityProbeKind {
        self.probe
    }
}

#[async_trait]
impl CapabilityProbeAdapter for ProviderCapabilityProbeAdapter<'_> {
    fn family(&self) -> ApiFamily {
        self.family
    }

    fn implemented_probes(&self) -> &'static [CapabilityProbeKind] {
        match self.probe {
            CapabilityProbeKind::Streaming => &[CapabilityProbeKind::Streaming],
            CapabilityProbeKind::Reasoning => &[CapabilityProbeKind::Reasoning],
            CapabilityProbeKind::PromptCaching => &[CapabilityProbeKind::PromptCaching],
            CapabilityProbeKind::StructuredOutput => &[CapabilityProbeKind::StructuredOutput],
            CapabilityProbeKind::ToolCalling => &[CapabilityProbeKind::ToolCalling],
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn execute_probe(
        &self,
        request: AdapterProbeRequest,
        cancelled: watch::Receiver<bool>,
    ) -> Result<AdapterProbeResult, ProbeAdapterError> {
        if request.model_route_id() != &self.model_route_id
            || request.adapter_family() != self.family
            || request.probe() != self.probe
        {
            return Err(ProbeAdapterError::InvalidRequest);
        }
        if self.call_cost_upper_bound_micro_usd > request.budget().max_cost_micro_usd() {
            return Err(ProbeAdapterError::InvalidRequest);
        }
        if *cancelled.borrow() {
            return Err(ProbeAdapterError::Interrupted);
        }

        let input_token_reservation = match self.probe {
            CapabilityProbeKind::StructuredOutput => STRUCTURED_PROBE_INPUT_TOKEN_RESERVATION,
            CapabilityProbeKind::ToolCalling => TOOL_PROBE_INPUT_TOKEN_RESERVATION,
            CapabilityProbeKind::Streaming
            | CapabilityProbeKind::Reasoning
            | CapabilityProbeKind::PromptCaching => BASIC_PROBE_INPUT_TOKEN_RESERVATION,
        };
        let available_output_tokens = request
            .budget()
            .max_total_tokens()
            .saturating_sub(input_token_reservation);
        let output_token_limit = request
            .budget()
            .max_output_tokens()
            .min(available_output_tokens)
            .min(PROBE_OUTPUT_TOKEN_LIMIT);
        let max_output_tokens = u32::try_from(output_token_limit)
            .ok()
            .filter(|limit| *limit > 0)
            .ok_or(ProbeAdapterError::InvalidRequest)?;

        let conversation_id = ConversationId::new();
        let prompt = match self.probe {
            CapabilityProbeKind::StructuredOutput => STRUCTURED_OUTPUT_PROBE_PROMPT,
            CapabilityProbeKind::ToolCalling => TOOL_CALL_PROBE_PROMPT,
            CapabilityProbeKind::Streaming
            | CapabilityProbeKind::Reasoning
            | CapabilityProbeKind::PromptCaching => SYNTHETIC_PROBE_PROMPT,
        };
        let generation_request = GenerationRequest {
            generation_id: GenerationId::new(),
            conversation_id: conversation_id.clone(),
            model: self.model_id.clone(),
            messages: vec![Message::user(conversation_id, prompt)],
            resolved_prompt_plan: None,
            provider_execution_plan_hash: None,
            temperature: None,
            max_output_tokens: Some(max_output_tokens),
            provider_provenance: None,
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        };
        let request_plan = match self.probe {
            CapabilityProbeKind::StructuredOutput => Some(capability_probe_request_plan(
                self.family,
                CapabilityProbePlanKind::StructuredOutput,
            )),
            CapabilityProbeKind::ToolCalling => Some(capability_probe_request_plan(
                self.family,
                CapabilityProbePlanKind::ToolCalling,
            )),
            CapabilityProbeKind::Streaming
            | CapabilityProbeKind::Reasoning
            | CapabilityProbeKind::PromptCaching => None,
        };
        let (sink, events) = mpsc::channel(PROBE_EVENT_CHANNEL_CAPACITY);
        let (usage, signals) = execute_provider_generation(
            self.provider.as_ref(),
            generation_request,
            self.credential,
            request_plan,
            sink,
            events,
            cancelled,
        )
        .await?;

        let input_tokens = usage
            .input_tokens
            .ok_or(ProbeAdapterError::ProtocolViolation)?;
        let output_tokens = usage
            .output_tokens
            .ok_or(ProbeAdapterError::ProtocolViolation)?;
        let evidence = match self.probe {
            CapabilityProbeKind::Streaming if signals.event_count > 0 => {
                ProbeEvidence::StreamingCompleted {
                    event_count: signals.event_count,
                }
            }
            CapabilityProbeKind::Reasoning
                if signals.saw_reasoning
                    || usage.reasoning_tokens.is_some_and(|value| value > 0) =>
            {
                ProbeEvidence::ReasoningObserved
            }
            CapabilityProbeKind::PromptCaching
                if usage.cached_read_tokens.is_some_and(|value| value > 0) =>
            {
                ProbeEvidence::PromptCacheHit {
                    cache_read_tokens: usage.cached_read_tokens.unwrap_or_default(),
                }
            }
            CapabilityProbeKind::StructuredOutput if signals.structured_output_is_exact() => {
                ProbeEvidence::StructuredOutputValidated
            }
            CapabilityProbeKind::ToolCalling if signals.tool_call_is_exact() => {
                ProbeEvidence::ToolCallRoundTrip
            }
            CapabilityProbeKind::Streaming
            | CapabilityProbeKind::Reasoning
            | CapabilityProbeKind::StructuredOutput
            | CapabilityProbeKind::ToolCalling
            | CapabilityProbeKind::PromptCaching => {
                return Err(ProbeAdapterError::ProtocolViolation);
            }
        };

        Ok(AdapterProbeResult::new(
            true,
            evidence,
            ProbeUsage::new(
                input_tokens,
                output_tokens,
                self.call_cost_upper_bound_micro_usd,
                PROBE_CALL_LIMIT,
            ),
        ))
    }
}

#[derive(Default)]
struct ProviderProbeSignals {
    event_count: u32,
    event_bytes: usize,
    saw_reasoning: bool,
    text: String,
    tool_call: Option<ProbeToolCallSignals>,
}

#[derive(Default)]
struct ProbeToolCallSignals {
    id: String,
    name: String,
    arguments: String,
    completed: bool,
}

impl ProviderProbeSignals {
    fn observe(&mut self, event: ProviderEvent) -> Result<(), ProbeAdapterError> {
        let (bytes, reasoning) = match &event {
            ProviderEvent::TextDelta(content) => (content.len(), false),
            ProviderEvent::ReasoningDelta(content) => (content.len(), true),
            ProviderEvent::OpaqueReasoningState(state) => (state.payload_bytes(), false),
            ProviderEvent::ToolCallStarted { id, name } => {
                (id.as_str().len().saturating_add(name.as_str().len()), false)
            }
            ProviderEvent::ToolCallArgumentsDelta { id, delta } => (
                id.as_str().len().saturating_add(delta.as_str().len()),
                false,
            ),
            ProviderEvent::ToolCallCompleted { id } => (id.as_str().len(), false),
        };
        self.event_count = self
            .event_count
            .checked_add(1)
            .filter(|count| *count <= MAX_PROBE_EVENT_COUNT)
            .ok_or(ProbeAdapterError::ProtocolViolation)?;
        self.event_bytes = self
            .event_bytes
            .checked_add(bytes)
            .filter(|total| *total <= MAX_PROBE_EVENT_BYTES)
            .ok_or(ProbeAdapterError::ProtocolViolation)?;
        self.saw_reasoning |= reasoning && bytes > 0;
        match event {
            ProviderEvent::TextDelta(content) => self.text.push_str(&content),
            ProviderEvent::ToolCallStarted { id, name } => {
                if self.tool_call.is_some() {
                    return Err(ProbeAdapterError::ProtocolViolation);
                }
                self.tool_call = Some(ProbeToolCallSignals {
                    id: id.into_inner(),
                    name: name.into_inner(),
                    arguments: String::new(),
                    completed: false,
                });
            }
            ProviderEvent::ToolCallArgumentsDelta { id, delta } => {
                let tool_call = self
                    .tool_call
                    .as_mut()
                    .filter(|tool_call| tool_call.id == id.as_str() && !tool_call.completed)
                    .ok_or(ProbeAdapterError::ProtocolViolation)?;
                tool_call.arguments.push_str(delta.as_str());
            }
            ProviderEvent::ToolCallCompleted { id } => {
                let tool_call = self
                    .tool_call
                    .as_mut()
                    .filter(|tool_call| tool_call.id == id.as_str() && !tool_call.completed)
                    .ok_or(ProbeAdapterError::ProtocolViolation)?;
                tool_call.completed = true;
            }
            ProviderEvent::ReasoningDelta(_) | ProviderEvent::OpaqueReasoningState(_) => {}
        }
        Ok(())
    }

    fn structured_output_is_exact(&self) -> bool {
        self.tool_call.is_none()
            && serde_json::from_str::<serde_json::Value>(self.text.trim())
                .is_ok_and(|value| value == serde_json::json!({"probe": "ok"}))
    }

    fn tool_call_is_exact(&self) -> bool {
        self.text.trim().is_empty()
            && self.tool_call.as_ref().is_some_and(|tool_call| {
                tool_call.completed
                    && tool_call.name == CAPABILITY_PROBE_TOOL_NAME
                    && serde_json::from_str::<serde_json::Value>(&tool_call.arguments)
                        .is_ok_and(|value| value == serde_json::json!({"probe": "ok"}))
            })
    }
}

async fn execute_provider_generation(
    provider: &dyn Provider,
    request: GenerationRequest,
    credential: Option<&str>,
    request_plan: Option<ProviderRequestPlan>,
    sink: mpsc::Sender<ProviderEvent>,
    mut events: mpsc::Receiver<ProviderEvent>,
    mut cancelled: watch::Receiver<bool>,
) -> Result<(GenerationUsage, ProviderProbeSignals), ProbeAdapterError> {
    let generation_cancelled = cancelled.clone();
    let generation = async move {
        if let Some(request_plan) = request_plan {
            provider
                .generate_with_internal_plan(
                    request,
                    credential,
                    sink,
                    generation_cancelled,
                    request_plan,
                )
                .await
        } else {
            provider
                .generate(request, credential, sink, generation_cancelled)
                .await
        }
    };
    tokio::pin!(generation);
    let mut signals = ProviderProbeSignals::default();
    let mut events_open = true;
    let mut cancellation_open = true;

    let usage = loop {
        tokio::select! {
            biased;
            change = cancelled.changed(), if cancellation_open => {
                match change {
                    Ok(()) if *cancelled.borrow() => {
                        return Err(ProbeAdapterError::Interrupted);
                    }
                    Ok(()) => {}
                    Err(_) => cancellation_open = false,
                }
            }
            event = events.recv(), if events_open => {
                match event {
                    Some(event) => signals.observe(event)?,
                    None => events_open = false,
                }
            }
            result = &mut generation => {
                break result.map_err(classify_provider_error)?;
            }
        }
    };
    while let Ok(event) = events.try_recv() {
        signals.observe(event)?;
    }
    Ok((usage, signals))
}

fn classify_provider_error(error: CoreError) -> ProbeAdapterError {
    match error.code {
        CoreErrorCode::ProviderAuthFailed | CoreErrorCode::PermissionDenied => {
            ProbeAdapterError::Authentication
        }
        CoreErrorCode::ProviderRateLimited => ProbeAdapterError::RateLimited,
        CoreErrorCode::ProviderUnavailable
        | CoreErrorCode::NetworkUnavailable
        | CoreErrorCode::NotFound => ProbeAdapterError::Unavailable,
        CoreErrorCode::InvalidInput | CoreErrorCode::UnsupportedContent => {
            ProbeAdapterError::InvalidRequest
        }
        CoreErrorCode::Cancelled => ProbeAdapterError::Interrupted,
        CoreErrorCode::UnsafeArchive
        | CoreErrorCode::StorageUnavailable
        | CoreErrorCode::StorageCorrupted
        | CoreErrorCode::Internal => ProbeAdapterError::ProtocolViolation,
    }
}

fn probe_not_implemented(message: &'static str) -> CoreError {
    CoreError::new(CoreErrorCode::UnsupportedContent, message, false)
}

/// A known probe failure that is safe to show and persist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailure {
    ConsentMismatch,
    ConsentAlreadyConsumed,
    ConsentLedgerFull,
    Authentication,
    RateLimited,
    ProviderUnavailable,
    InvalidRequest,
    ProtocolViolation,
    BudgetExceeded,
}

impl ProbeFailure {
    pub fn recoverable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::ProviderUnavailable | Self::BudgetExceeded
        )
    }
}

/// Why the engine cannot safely state whether the probe took effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownOutcomeReason {
    CancelledAfterStart,
    TimedOut,
    AdapterInterrupted,
}

/// An interrupted single attempt. Retrying requires fresh explicit consent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownProbeOutcome {
    model_route_id: ModelRouteId,
    probe: CapabilityProbeKind,
    reason: UnknownOutcomeReason,
    calls_started: u32,
}

impl UnknownProbeOutcome {
    pub fn model_route_id(&self) -> &ModelRouteId {
        &self.model_route_id
    }

    pub fn probe(&self) -> CapabilityProbeKind {
        self.probe
    }

    pub fn reason(&self) -> UnknownOutcomeReason {
        self.reason
    }

    pub fn calls_started(&self) -> u32 {
        self.calls_started
    }
}

/// Terminal result for one explicitly authorized probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeRunOutcome {
    Observed(CapabilityObservation),
    Failed(ProbeFailure),
    UnknownOutcome(UnknownProbeOutcome),
    CancelledBeforeStart,
}

/// Executes one-shot capability probes and prevents consent replay.
pub struct CapabilityProbeEngine {
    consumed_consents: Mutex<HashSet<String>>,
}

impl Default for CapabilityProbeEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilityProbeEngine {
    pub fn new() -> Self {
        Self {
            consumed_consents: Mutex::new(HashSet::new()),
        }
    }

    pub async fn run<'adapter>(
        &self,
        adapter: Arc<dyn CapabilityProbeAdapter + 'adapter>,
        expected_route_id: &ModelRouteId,
        expected_probe: CapabilityProbeKind,
        consent: ProbeConsent,
        cancelled: watch::Receiver<bool>,
    ) -> ProbeRunOutcome {
        if *cancelled.borrow() {
            return ProbeRunOutcome::CancelledBeforeStart;
        }
        if consent.model_route_id() != expected_route_id || consent.probe() != expected_probe {
            return ProbeRunOutcome::Failed(ProbeFailure::ConsentMismatch);
        }

        {
            let mut consumed = self.consumed_consents.lock().await;
            if consumed.contains(consent.id()) {
                return ProbeRunOutcome::Failed(ProbeFailure::ConsentAlreadyConsumed);
            }
            if consumed.len() >= MAX_CONSUMED_CONSENTS {
                return ProbeRunOutcome::Failed(ProbeFailure::ConsentLedgerFull);
            }
            consumed.insert(consent.id().to_owned());
        }

        let family = adapter.family();
        let request = AdapterProbeRequest {
            model_route_id: expected_route_id.clone(),
            adapter_family: family,
            probe: expected_probe,
            budget: consent.budget(),
        };
        let budget = consent.budget();
        let adapter_cancelled = cancelled.clone();
        let operation = adapter.execute_probe(request, adapter_cancelled);
        tokio::pin!(operation);
        let timeout = tokio::time::sleep(budget.max_duration());
        tokio::pin!(timeout);
        let cancellation = wait_for_cancellation(cancelled);
        tokio::pin!(cancellation);

        let result = tokio::select! {
            biased;
            result = &mut operation => result,
            () = &mut cancellation => {
                return ProbeRunOutcome::UnknownOutcome(UnknownProbeOutcome {
                    model_route_id: expected_route_id.clone(),
                    probe: expected_probe,
                    reason: UnknownOutcomeReason::CancelledAfterStart,
                    calls_started: PROBE_CALL_LIMIT,
                });
            }
            () = &mut timeout => {
                return ProbeRunOutcome::UnknownOutcome(UnknownProbeOutcome {
                    model_route_id: expected_route_id.clone(),
                    probe: expected_probe,
                    reason: UnknownOutcomeReason::TimedOut,
                    calls_started: PROBE_CALL_LIMIT,
                });
            }
        };

        match result {
            Ok(result) => {
                if validate_adapter_result(expected_probe, &result, budget).is_err() {
                    return ProbeRunOutcome::Failed(
                        if usage_within_budget(result.usage(), budget) {
                            ProbeFailure::ProtocolViolation
                        } else {
                            ProbeFailure::BudgetExceeded
                        },
                    );
                }
                ProbeRunOutcome::Observed(observation_from_probe(
                    expected_route_id.clone(),
                    expected_probe,
                    consent.id(),
                    result.supported(),
                ))
            }
            Err(ProbeAdapterError::Interrupted) => {
                ProbeRunOutcome::UnknownOutcome(UnknownProbeOutcome {
                    model_route_id: expected_route_id.clone(),
                    probe: expected_probe,
                    reason: UnknownOutcomeReason::AdapterInterrupted,
                    calls_started: PROBE_CALL_LIMIT,
                })
            }
            Err(error) => ProbeRunOutcome::Failed(match error {
                ProbeAdapterError::Authentication => ProbeFailure::Authentication,
                ProbeAdapterError::RateLimited => ProbeFailure::RateLimited,
                ProbeAdapterError::Unavailable => ProbeFailure::ProviderUnavailable,
                ProbeAdapterError::InvalidRequest => ProbeFailure::InvalidRequest,
                ProbeAdapterError::ProtocolViolation => ProbeFailure::ProtocolViolation,
                ProbeAdapterError::Interrupted => unreachable!("handled above"),
            }),
        }
    }
}

async fn wait_for_cancellation(mut cancelled: watch::Receiver<bool>) {
    loop {
        if *cancelled.borrow() {
            return;
        }
        if cancelled.changed().await.is_err() {
            pending::<()>().await;
        }
    }
}

fn validate_adapter_result(
    probe: CapabilityProbeKind,
    result: &AdapterProbeResult,
    budget: ProbeBudget,
) -> CoreResult<()> {
    if !usage_within_budget(result.usage(), budget) {
        return Err(CoreError::invalid(
            "capability probe adapter exceeded its authorized budget",
        ));
    }
    if result.supported() {
        let valid_evidence = matches!(
            (probe, result.evidence()),
            (
                CapabilityProbeKind::Streaming,
                ProbeEvidence::StreamingCompleted { event_count: 1.. }
            ) | (
                CapabilityProbeKind::Reasoning,
                ProbeEvidence::ReasoningObserved
            ) | (
                CapabilityProbeKind::StructuredOutput,
                ProbeEvidence::StructuredOutputValidated
            ) | (
                CapabilityProbeKind::ToolCalling,
                ProbeEvidence::ToolCallRoundTrip
            ) | (
                CapabilityProbeKind::PromptCaching,
                ProbeEvidence::PromptCacheHit {
                    cache_read_tokens: 1..
                }
            )
        );
        if !valid_evidence {
            return Err(CoreError::invalid(
                "capability probe returned evidence for a different capability",
            ));
        }
    } else if result.evidence() != &ProbeEvidence::ExplicitlyUnsupported {
        return Err(CoreError::invalid(
            "negative capability result requires explicit unsupported evidence",
        ));
    }
    Ok(())
}

fn usage_within_budget(usage: ProbeUsage, budget: ProbeBudget) -> bool {
    usage.calls() == PROBE_CALL_LIMIT
        && usage.calls() <= budget.max_calls()
        && usage
            .input_tokens()
            .checked_add(usage.output_tokens())
            .is_some_and(|total| total <= budget.max_total_tokens())
        && usage.output_tokens() <= budget.max_output_tokens()
        && usage.cost_micro_usd() <= budget.max_cost_micro_usd()
}

fn observation_from_probe(
    model_route_id: ModelRouteId,
    probe: CapabilityProbeKind,
    consent_id: &str,
    supported: bool,
) -> CapabilityObservation {
    let observed_at = Utc::now();
    let freshness_seconds = if probe == CapabilityProbeKind::PromptCaching {
        CACHE_OBSERVATION_FRESHNESS_SECONDS
    } else {
        DEFAULT_OBSERVATION_FRESHNESS_SECONDS
    };
    let status = if supported {
        SupportStatus::Verified
    } else {
        SupportStatus::Unsupported
    };
    CapabilityObservation {
        id: ObservationId::from(format!("observation:{consent_id}")),
        model_route_id,
        key: probe.into(),
        value: CapabilityValue::Boolean(supported),
        status,
        source: ObservationSource::CapabilityProbe,
        confidence: if supported {
            Confidence::High
        } else {
            Confidence::Medium
        },
        observed_at,
        expires_at: observed_at.checked_add_signed(chrono::Duration::seconds(freshness_seconds)),
        evidence_ref: Some(EvidenceId::from(format!("probe:{consent_id}"))),
    }
}

/// A deterministic merge that keeps conflicts visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedCapabilityObservation {
    selected: CapabilityObservation,
    alternatives: Vec<CapabilityObservation>,
    selected_is_stale: bool,
    has_conflict: bool,
}

impl MergedCapabilityObservation {
    pub fn selected(&self) -> &CapabilityObservation {
        &self.selected
    }

    pub fn alternatives(&self) -> &[CapabilityObservation] {
        &self.alternatives
    }

    pub fn selected_is_stale(&self) -> bool {
        self.selected_is_stale
    }

    pub fn has_conflict(&self) -> bool {
        self.has_conflict
    }
}

/// Merges observations for exactly one route and capability.
///
/// Fresh evidence wins over expired evidence, then source precedence,
/// confidence, observation time, and finally stable observation ID.
pub fn merge_capability_observations(
    observations: &[CapabilityObservation],
    now: DateTime<Utc>,
) -> CoreResult<MergedCapabilityObservation> {
    let Some(first) = observations.first() else {
        return Err(CoreError::invalid(
            "cannot merge an empty capability observation set",
        ));
    };
    if observations.len() > MAX_MERGE_OBSERVATIONS {
        return Err(CoreError::invalid(format!(
            "capability merge exceeds {MAX_MERGE_OBSERVATIONS} observations"
        )));
    }
    let mut ids = HashSet::with_capacity(observations.len());
    for observation in observations {
        if observation.model_route_id != first.model_route_id || observation.key != first.key {
            return Err(CoreError::invalid(
                "capability merge requires one route and one capability key",
            ));
        }
        if !ids.insert(observation.id.as_str()) {
            return Err(CoreError::invalid(
                "capability merge received a duplicate observation ID",
            ));
        }
    }

    let mut ranked = observations.to_vec();
    ranked.sort_unstable_by(|left, right| {
        observation_rank(right, now)
            .cmp(&observation_rank(left, now))
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });
    let selected = ranked.remove(0);
    let has_conflict = ranked
        .iter()
        .filter(|candidate| candidate.is_fresh_at(now))
        .any(|candidate| candidate.status != selected.status || candidate.value != selected.value);
    Ok(MergedCapabilityObservation {
        selected_is_stale: !selected.is_fresh_at(now),
        selected,
        alternatives: ranked,
        has_conflict,
    })
}

fn observation_rank(
    observation: &CapabilityObservation,
    now: DateTime<Utc>,
) -> (bool, u8, u8, DateTime<Utc>) {
    (
        observation.is_fresh_at(now),
        source_priority(observation.key, observation.source),
        confidence_priority(observation.confidence),
        observation.observed_at,
    )
}

fn source_priority(key: CapabilityKey, source: ObservationSource) -> u8 {
    match source {
        ObservationSource::UserOverride => 6,
        ObservationSource::ProviderApi
            if matches!(
                key,
                CapabilityKey::ContextWindow | CapabilityKey::MaxOutputTokens
            ) =>
        {
            5
        }
        ObservationSource::CapabilityProbe => {
            if matches!(
                key,
                CapabilityKey::ContextWindow | CapabilityKey::MaxOutputTokens
            ) {
                4
            } else {
                5
            }
        }
        ObservationSource::ProviderApi => 4,
        ObservationSource::OfficialDocumentation => 3,
        ObservationSource::SignedLorepiaCatalog => 2,
        ObservationSource::LlmInference => 1,
    }
}

fn confidence_priority(confidence: Confidence) -> u8 {
    match confidence {
        Confidence::Low => 1,
        Confidence::Medium => 2,
        Confidence::High => 3,
    }
}

fn is_canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
            }
        })
}

#[cfg(test)]
mod tests {
    use std::{
        future::pending,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use async_trait::async_trait;
    use chrono::{DateTime, TimeZone, Utc};
    use lorepia_domain::{
        ApiFamily, CapabilityKey, CapabilityObservation, CapabilityValue, Confidence, CoreError,
        CoreResult, EvidenceId, GenerationRequest, GenerationUsage, ModelRouteId, ObservationId,
        ObservationSource, ProviderCapabilities, SupportStatus, ToolCallArgumentsDelta, ToolCallId,
        ToolName,
    };
    use tokio::sync::watch;

    use super::{
        AdapterProbeRequest, AdapterProbeResult, CapabilityProbeAdapter, CapabilityProbeEngine,
        CapabilityProbeKind, ProbeAdapterError, ProbeBudget, ProbeConsent, ProbeEvidence,
        ProbeFailure, ProbeRunOutcome, ProbeUsage, ProviderCapabilityProbeAdapter,
        UnknownOutcomeReason, merge_capability_observations,
    };
    use crate::{
        Provider, ProviderEvent, ProviderEventSender, parameter_mapping::ProviderRequestPlan,
    };

    const CONSENT_A: &str = "00000000-0000-4000-8000-000000000001";
    const CONSENT_B: &str = "00000000-0000-4000-8000-000000000002";
    const CONSENT_C: &str = "00000000-0000-4000-8000-000000000003";

    #[derive(Debug, Clone, Copy)]
    enum SyntheticBehavior {
        Supported,
        Unsupported,
        Error(ProbeAdapterError),
        OverBudget,
        WrongEvidence,
        Hang,
    }

    struct SyntheticAdapter {
        calls: AtomicUsize,
        behavior: SyntheticBehavior,
    }

    #[derive(Clone, Copy)]
    enum ExactProviderBehavior {
        Exact,
        PromptOnlyTool,
        StructuredExtraField,
    }

    struct ExactPlanProvider {
        family: ApiFamily,
        behavior: ExactProviderBehavior,
        calls: AtomicUsize,
    }

    impl ExactPlanProvider {
        fn new(family: ApiFamily, behavior: ExactProviderBehavior) -> Self {
            Self {
                family,
                behavior,
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl Provider for ExactPlanProvider {
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
            _sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            Err(CoreError::internal(
                "structured/tool probes must use an internal plan",
            ))
        }

        async fn generate_with_internal_plan(
            &self,
            request: GenerationRequest,
            credential: Option<&str>,
            sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
            request_plan: ProviderRequestPlan,
        ) -> CoreResult<GenerationUsage> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if request_plan.family() != self.family
                || request.model != "fixture-model"
                || credential != Some("probe-secret")
            {
                return Err(CoreError::invalid("exact probe binding mismatch"));
            }
            let is_tool = request_plan
                .body_patches()
                .iter()
                .any(|patch| patch.path() == "tools");
            match (is_tool, self.behavior) {
                (true, ExactProviderBehavior::Exact) => {
                    let id = ToolCallId::parse("probe-call-1").expect("tool call ID");
                    sink.send(ProviderEvent::ToolCallStarted {
                        id: id.clone(),
                        name: ToolName::parse("lorepia_probe").expect("tool name"),
                    })
                    .await
                    .map_err(|_| CoreError::internal("probe event receiver closed"))?;
                    sink.send(ProviderEvent::ToolCallArgumentsDelta {
                        id: id.clone(),
                        delta: ToolCallArgumentsDelta::parse(r#"{"probe":"ok"}"#)
                            .expect("arguments"),
                    })
                    .await
                    .map_err(|_| CoreError::internal("probe event receiver closed"))?;
                    sink.send(ProviderEvent::ToolCallCompleted { id })
                        .await
                        .map_err(|_| CoreError::internal("probe event receiver closed"))?;
                }
                (true, ExactProviderBehavior::PromptOnlyTool) => {
                    sink.send(ProviderEvent::TextDelta(r#"{"probe":"ok"}"#.to_owned()))
                        .await
                        .map_err(|_| CoreError::internal("probe event receiver closed"))?;
                }
                (false, ExactProviderBehavior::StructuredExtraField) => {
                    sink.send(ProviderEvent::TextDelta(
                        r#"{"probe":"ok","extra":true}"#.to_owned(),
                    ))
                    .await
                    .map_err(|_| CoreError::internal("probe event receiver closed"))?;
                }
                (false, ExactProviderBehavior::Exact) => {
                    sink.send(ProviderEvent::TextDelta(r#"{"probe":"ok"}"#.to_owned()))
                        .await
                        .map_err(|_| CoreError::internal("probe event receiver closed"))?;
                }
                (false, ExactProviderBehavior::PromptOnlyTool)
                | (true, ExactProviderBehavior::StructuredExtraField) => {
                    return Err(CoreError::internal("invalid exact provider fixture"));
                }
            }
            Ok(GenerationUsage {
                input_tokens: Some(20),
                output_tokens: Some(4),
                ..GenerationUsage::default()
            })
        }
    }

    impl SyntheticAdapter {
        fn new(behavior: SyntheticBehavior) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                behavior,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl CapabilityProbeAdapter for SyntheticAdapter {
        fn family(&self) -> ApiFamily {
            ApiFamily::OpenAiResponses
        }

        async fn execute_probe(
            &self,
            request: AdapterProbeRequest,
            _cancelled: watch::Receiver<bool>,
        ) -> Result<AdapterProbeResult, ProbeAdapterError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.behavior {
                SyntheticBehavior::Supported => Ok(AdapterProbeResult::new(
                    true,
                    supported_evidence(request.probe()),
                    ProbeUsage::new(40, 12, 100, 1),
                )),
                SyntheticBehavior::Unsupported => Ok(AdapterProbeResult::new(
                    false,
                    ProbeEvidence::ExplicitlyUnsupported,
                    ProbeUsage::new(20, 2, 50, 1),
                )),
                SyntheticBehavior::Error(error) => Err(error),
                SyntheticBehavior::OverBudget => Ok(AdapterProbeResult::new(
                    true,
                    supported_evidence(request.probe()),
                    ProbeUsage::new(10_000, 2_000, 1_000_000, 2),
                )),
                SyntheticBehavior::WrongEvidence => Ok(AdapterProbeResult::new(
                    true,
                    ProbeEvidence::ToolCallRoundTrip,
                    ProbeUsage::new(40, 12, 100, 1),
                )),
                SyntheticBehavior::Hang => {
                    pending::<Result<AdapterProbeResult, ProbeAdapterError>>().await
                }
            }
        }
    }

    fn supported_evidence(probe: CapabilityProbeKind) -> ProbeEvidence {
        match probe {
            CapabilityProbeKind::Streaming => ProbeEvidence::StreamingCompleted { event_count: 2 },
            CapabilityProbeKind::Reasoning => ProbeEvidence::ReasoningObserved,
            CapabilityProbeKind::StructuredOutput => ProbeEvidence::StructuredOutputValidated,
            CapabilityProbeKind::ToolCalling => ProbeEvidence::ToolCallRoundTrip,
            CapabilityProbeKind::PromptCaching => ProbeEvidence::PromptCacheHit {
                cache_read_tokens: 16,
            },
        }
    }

    fn budget(duration: Duration) -> ProbeBudget {
        ProbeBudget::new(512, 64, 1_000, duration, 1).expect("valid budget")
    }

    fn consent(
        id: &str,
        route: &ModelRouteId,
        probe: CapabilityProbeKind,
        duration: Duration,
    ) -> ProbeConsent {
        ProbeConsent::new(id, route.clone(), probe, budget(duration)).expect("valid consent")
    }

    #[tokio::test]
    async fn all_five_probes_emit_fresh_verified_observations() {
        for (index, probe) in [
            CapabilityProbeKind::Streaming,
            CapabilityProbeKind::Reasoning,
            CapabilityProbeKind::StructuredOutput,
            CapabilityProbeKind::ToolCalling,
            CapabilityProbeKind::PromptCaching,
        ]
        .into_iter()
        .enumerate()
        {
            let adapter = Arc::new(SyntheticAdapter::new(SyntheticBehavior::Supported));
            let engine = CapabilityProbeEngine::new();
            let route = ModelRouteId::from("route-a");
            let consent_id = format!("00000000-0000-4000-8000-{index:012x}");
            let (_sender, receiver) = watch::channel(false);
            let outcome = engine
                .run(
                    adapter.clone(),
                    &route,
                    probe,
                    consent(&consent_id, &route, probe, Duration::from_secs(1)),
                    receiver,
                )
                .await;

            let ProbeRunOutcome::Observed(observation) = outcome else {
                panic!("expected an observation");
            };
            assert_eq!(observation.key, CapabilityKey::from(probe));
            assert_eq!(observation.value, CapabilityValue::Boolean(true));
            assert_eq!(observation.status, SupportStatus::Verified);
            assert_eq!(observation.confidence, Confidence::High);
            assert_eq!(observation.source, ObservationSource::CapabilityProbe);
            assert!(observation.is_fresh_at(Utc::now()));
            assert_eq!(adapter.calls(), 1);
        }
    }

    #[tokio::test]
    async fn provider_adapter_executes_exact_structured_and_tool_plans_for_every_family() {
        let engine = CapabilityProbeEngine::new();
        let route = ModelRouteId::from("route-a");
        let mut index = 0_u64;
        for family in [
            ApiFamily::OpenAiResponses,
            ApiFamily::OpenAiChatCompletions,
            ApiFamily::AnthropicMessages,
            ApiFamily::GeminiGenerateContent,
            ApiFamily::OllamaNative,
        ] {
            for probe in [
                CapabilityProbeKind::StructuredOutput,
                CapabilityProbeKind::ToolCalling,
            ] {
                index += 1;
                let exact = Arc::new(ExactPlanProvider::new(family, ExactProviderBehavior::Exact));
                let provider: Arc<dyn Provider> = exact.clone();
                let adapter = ProviderCapabilityProbeAdapter::new(
                    family,
                    route.clone(),
                    "fixture-model",
                    provider,
                    Some("probe-secret"),
                    probe,
                    100,
                )
                .expect("production probe adapter");
                assert_eq!(adapter.implemented_probes(), &[probe]);
                let consent_id = format!("00000000-0000-4000-8000-{index:012x}");
                let (_sender, receiver) = watch::channel(false);
                let outcome = engine
                    .run(
                        Arc::new(adapter),
                        &route,
                        probe,
                        consent(&consent_id, &route, probe, Duration::from_secs(1)),
                        receiver,
                    )
                    .await;
                let ProbeRunOutcome::Observed(observation) = outcome else {
                    panic!("expected exact provider observation, got {outcome:?}");
                };
                assert_eq!(observation.status, SupportStatus::Verified);
                assert_eq!(observation.value, CapabilityValue::Boolean(true));
                assert_eq!(exact.calls.load(Ordering::SeqCst), 1);
            }
        }
    }

    #[tokio::test]
    async fn prompt_only_tool_and_schema_superset_never_count_as_exact_evidence() {
        for (index, probe, behavior) in [
            (
                1_u64,
                CapabilityProbeKind::ToolCalling,
                ExactProviderBehavior::PromptOnlyTool,
            ),
            (
                2_u64,
                CapabilityProbeKind::StructuredOutput,
                ExactProviderBehavior::StructuredExtraField,
            ),
        ] {
            let route = ModelRouteId::from("route-a");
            let provider: Arc<dyn Provider> =
                Arc::new(ExactPlanProvider::new(ApiFamily::OpenAiResponses, behavior));
            let adapter = ProviderCapabilityProbeAdapter::new(
                ApiFamily::OpenAiResponses,
                route.clone(),
                "fixture-model",
                provider,
                Some("probe-secret"),
                probe,
                100,
            )
            .expect("production probe adapter");
            let consent_id = format!("00000000-0000-4000-8000-{index:012x}");
            let (_sender, receiver) = watch::channel(false);
            let outcome = CapabilityProbeEngine::new()
                .run(
                    Arc::new(adapter),
                    &route,
                    probe,
                    consent(&consent_id, &route, probe, Duration::from_secs(1)),
                    receiver,
                )
                .await;
            assert_eq!(
                outcome,
                ProbeRunOutcome::Failed(ProbeFailure::ProtocolViolation)
            );
        }
    }

    #[tokio::test]
    async fn exact_probe_rejects_an_under_reserved_token_budget_before_provider_call() {
        let route = ModelRouteId::from("route-a");
        let exact = Arc::new(ExactPlanProvider::new(
            ApiFamily::OpenAiResponses,
            ExactProviderBehavior::Exact,
        ));
        let provider: Arc<dyn Provider> = exact.clone();
        let adapter = ProviderCapabilityProbeAdapter::new(
            ApiFamily::OpenAiResponses,
            route.clone(),
            "fixture-model",
            provider,
            Some("probe-secret"),
            CapabilityProbeKind::ToolCalling,
            100,
        )
        .expect("production probe adapter");
        let consent = ProbeConsent::new(
            CONSENT_A,
            route.clone(),
            CapabilityProbeKind::ToolCalling,
            ProbeBudget::new(256, 32, 1_000, Duration::from_secs(1), 1)
                .expect("syntactically valid but under-reserved budget"),
        )
        .expect("consent");
        let (_sender, receiver) = watch::channel(false);
        let outcome = CapabilityProbeEngine::new()
            .run(
                Arc::new(adapter),
                &route,
                CapabilityProbeKind::ToolCalling,
                consent,
                receiver,
            )
            .await;
        assert_eq!(
            outcome,
            ProbeRunOutcome::Failed(ProbeFailure::InvalidRequest)
        );
        assert_eq!(exact.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn negative_probe_requires_explicit_evidence() {
        let adapter = Arc::new(SyntheticAdapter::new(SyntheticBehavior::Unsupported));
        let engine = CapabilityProbeEngine::new();
        let route = ModelRouteId::from("route-a");
        let (_sender, receiver) = watch::channel(false);
        let outcome = engine
            .run(
                adapter.clone(),
                &route,
                CapabilityProbeKind::Reasoning,
                consent(
                    CONSENT_A,
                    &route,
                    CapabilityProbeKind::Reasoning,
                    Duration::from_secs(1),
                ),
                receiver,
            )
            .await;

        let ProbeRunOutcome::Observed(observation) = outcome else {
            panic!("expected an observation");
        };
        assert_eq!(observation.status, SupportStatus::Unsupported);
        assert_eq!(observation.value, CapabilityValue::Boolean(false));
        assert_eq!(observation.confidence, Confidence::Medium);
        assert_eq!(adapter.calls(), 1);
    }

    #[tokio::test]
    async fn consent_is_bound_to_route_and_probe_and_cannot_be_replayed() {
        let adapter = Arc::new(SyntheticAdapter::new(SyntheticBehavior::Supported));
        let engine = CapabilityProbeEngine::new();
        let route = ModelRouteId::from("route-a");
        let other_route = ModelRouteId::from("route-b");
        let (_sender, receiver) = watch::channel(false);
        let mismatch = engine
            .run(
                adapter.clone(),
                &other_route,
                CapabilityProbeKind::Streaming,
                consent(
                    CONSENT_A,
                    &route,
                    CapabilityProbeKind::Streaming,
                    Duration::from_secs(1),
                ),
                receiver,
            )
            .await;
        assert_eq!(
            mismatch,
            ProbeRunOutcome::Failed(ProbeFailure::ConsentMismatch)
        );
        assert_eq!(adapter.calls(), 0);

        let (_sender, receiver) = watch::channel(false);
        let first = engine
            .run(
                adapter.clone(),
                &route,
                CapabilityProbeKind::Streaming,
                consent(
                    CONSENT_B,
                    &route,
                    CapabilityProbeKind::Streaming,
                    Duration::from_secs(1),
                ),
                receiver,
            )
            .await;
        assert!(matches!(first, ProbeRunOutcome::Observed(_)));

        let (_sender, receiver) = watch::channel(false);
        let replay = engine
            .run(
                adapter.clone(),
                &route,
                CapabilityProbeKind::Streaming,
                consent(
                    CONSENT_B,
                    &route,
                    CapabilityProbeKind::Streaming,
                    Duration::from_secs(1),
                ),
                receiver,
            )
            .await;
        assert_eq!(
            replay,
            ProbeRunOutcome::Failed(ProbeFailure::ConsentAlreadyConsumed)
        );
        assert_eq!(adapter.calls(), 1);
    }

    #[test]
    fn budgets_enforce_token_cost_time_and_single_call_hard_caps() {
        assert!(ProbeBudget::new(0, 1, 0, Duration::from_secs(1), 1).is_err());
        assert!(ProbeBudget::new(4_097, 1, 0, Duration::from_secs(1), 1).is_err());
        assert!(ProbeBudget::new(100, 101, 0, Duration::from_secs(1), 1).is_err());
        assert!(ProbeBudget::new(100, 10, 100_001, Duration::from_secs(1), 1).is_err());
        assert!(ProbeBudget::new(100, 10, 0, Duration::ZERO, 1).is_err());
        assert!(ProbeBudget::new(100, 10, 0, Duration::from_secs(61), 1).is_err());
        assert!(ProbeBudget::new(100, 10, 0, Duration::from_secs(1), 2).is_err());
        assert!(
            ProbeConsent::new(
                "sk-not-a-consent-secret",
                ModelRouteId::from("route"),
                CapabilityProbeKind::Streaming,
                budget(Duration::from_secs(1))
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn budget_overrun_and_wrong_evidence_fail_without_retry() {
        for (id, behavior, expected) in [
            (
                CONSENT_A,
                SyntheticBehavior::OverBudget,
                ProbeFailure::BudgetExceeded,
            ),
            (
                CONSENT_B,
                SyntheticBehavior::WrongEvidence,
                ProbeFailure::ProtocolViolation,
            ),
        ] {
            let adapter = Arc::new(SyntheticAdapter::new(behavior));
            let engine = CapabilityProbeEngine::new();
            let route = ModelRouteId::from("route-a");
            let (_sender, receiver) = watch::channel(false);
            let outcome = engine
                .run(
                    adapter.clone(),
                    &route,
                    CapabilityProbeKind::Streaming,
                    consent(
                        id,
                        &route,
                        CapabilityProbeKind::Streaming,
                        Duration::from_secs(1),
                    ),
                    receiver,
                )
                .await;
            assert_eq!(outcome, ProbeRunOutcome::Failed(expected));
            assert_eq!(adapter.calls(), 1);
        }
    }

    #[tokio::test]
    async fn recoverable_adapter_error_is_not_automatically_retried() {
        let adapter = Arc::new(SyntheticAdapter::new(SyntheticBehavior::Error(
            ProbeAdapterError::RateLimited,
        )));
        let engine = CapabilityProbeEngine::new();
        let route = ModelRouteId::from("route-a");
        let (_sender, receiver) = watch::channel(false);
        let outcome = engine
            .run(
                adapter.clone(),
                &route,
                CapabilityProbeKind::Streaming,
                consent(
                    CONSENT_A,
                    &route,
                    CapabilityProbeKind::Streaming,
                    Duration::from_secs(1),
                ),
                receiver,
            )
            .await;
        assert_eq!(outcome, ProbeRunOutcome::Failed(ProbeFailure::RateLimited));
        assert!(ProbeFailure::RateLimited.recoverable());
        assert_eq!(adapter.calls(), 1);
    }

    #[tokio::test]
    async fn pre_cancel_spends_nothing_but_midflight_cancel_is_unknown() {
        let adapter = Arc::new(SyntheticAdapter::new(SyntheticBehavior::Hang));
        let engine = Arc::new(CapabilityProbeEngine::new());
        let route = ModelRouteId::from("route-a");

        let (_sender, receiver) = watch::channel(true);
        let outcome = engine
            .run(
                adapter.clone(),
                &route,
                CapabilityProbeKind::Streaming,
                consent(
                    CONSENT_A,
                    &route,
                    CapabilityProbeKind::Streaming,
                    Duration::from_secs(1),
                ),
                receiver,
            )
            .await;
        assert_eq!(outcome, ProbeRunOutcome::CancelledBeforeStart);
        assert_eq!(adapter.calls(), 0);

        let (sender, receiver) = watch::channel(false);
        let task = {
            let engine = engine.clone();
            let adapter = adapter.clone();
            let route = route.clone();
            tokio::spawn(async move {
                engine
                    .run(
                        adapter,
                        &route,
                        CapabilityProbeKind::Streaming,
                        consent(
                            CONSENT_B,
                            &route,
                            CapabilityProbeKind::Streaming,
                            Duration::from_secs(1),
                        ),
                        receiver,
                    )
                    .await
            })
        };
        tokio::task::yield_now().await;
        sender.send(true).expect("send cancellation");
        let outcome = task.await.expect("probe task");
        let ProbeRunOutcome::UnknownOutcome(unknown) = outcome else {
            panic!("midflight cancellation must be unknown");
        };
        assert_eq!(unknown.reason(), UnknownOutcomeReason::CancelledAfterStart);
        assert_eq!(unknown.calls_started(), 1);
        assert_eq!(adapter.calls(), 1);
    }

    #[tokio::test]
    async fn timeout_and_adapter_interruption_are_unknown_and_never_retried() {
        let route = ModelRouteId::from("route-a");
        let engine = CapabilityProbeEngine::new();
        let hanging = Arc::new(SyntheticAdapter::new(SyntheticBehavior::Hang));
        let (_sender, receiver) = watch::channel(false);
        let timeout = engine
            .run(
                hanging.clone(),
                &route,
                CapabilityProbeKind::PromptCaching,
                consent(
                    CONSENT_A,
                    &route,
                    CapabilityProbeKind::PromptCaching,
                    Duration::from_millis(5),
                ),
                receiver,
            )
            .await;
        let ProbeRunOutcome::UnknownOutcome(unknown) = timeout else {
            panic!("timeout must be unknown");
        };
        assert_eq!(unknown.reason(), UnknownOutcomeReason::TimedOut);
        assert_eq!(hanging.calls(), 1);

        let interrupted = Arc::new(SyntheticAdapter::new(SyntheticBehavior::Error(
            ProbeAdapterError::Interrupted,
        )));
        let (_sender, receiver) = watch::channel(false);
        let outcome = engine
            .run(
                interrupted.clone(),
                &route,
                CapabilityProbeKind::PromptCaching,
                consent(
                    CONSENT_B,
                    &route,
                    CapabilityProbeKind::PromptCaching,
                    Duration::from_secs(1),
                ),
                receiver,
            )
            .await;
        let ProbeRunOutcome::UnknownOutcome(unknown) = outcome else {
            panic!("adapter interruption must be unknown");
        };
        assert_eq!(unknown.reason(), UnknownOutcomeReason::AdapterInterrupted);
        assert_eq!(interrupted.calls(), 1);
    }

    fn observation(
        id: &str,
        status: SupportStatus,
        source: ObservationSource,
        confidence: Confidence,
        observed_at: DateTime<Utc>,
        expires_at: Option<DateTime<Utc>>,
    ) -> CapabilityObservation {
        let supported = status == SupportStatus::Verified;
        CapabilityObservation {
            id: ObservationId::from(id),
            model_route_id: ModelRouteId::from("route-a"),
            key: CapabilityKey::Streaming,
            value: CapabilityValue::Boolean(supported),
            status,
            source,
            confidence,
            observed_at,
            expires_at,
            evidence_ref: Some(EvidenceId::from(format!("evidence:{id}"))),
        }
    }

    #[test]
    fn observation_merge_prefers_fresh_provenance_and_preserves_conflict() {
        let now = Utc
            .timestamp_opt(10_000, 0)
            .single()
            .expect("valid timestamp");
        let stale_override = observation(
            "override",
            SupportStatus::Verified,
            ObservationSource::UserOverride,
            Confidence::High,
            now - chrono::Duration::seconds(100),
            Some(now - chrono::Duration::seconds(1)),
        );
        let documentation = observation(
            "docs",
            SupportStatus::Documented,
            ObservationSource::OfficialDocumentation,
            Confidence::High,
            now - chrono::Duration::seconds(20),
            Some(now + chrono::Duration::seconds(100)),
        );
        let probe = observation(
            "probe",
            SupportStatus::Unsupported,
            ObservationSource::CapabilityProbe,
            Confidence::High,
            now - chrono::Duration::seconds(10),
            Some(now + chrono::Duration::seconds(100)),
        );

        let merged =
            merge_capability_observations(&[stale_override, documentation, probe.clone()], now)
                .expect("merge observations");
        assert_eq!(merged.selected(), &probe);
        assert!(!merged.selected_is_stale());
        assert!(merged.has_conflict());
        assert_eq!(merged.alternatives().len(), 2);
    }

    #[test]
    fn observation_merge_is_deterministic_and_rejects_mixed_groups() {
        let now = Utc
            .timestamp_opt(10_000, 0)
            .single()
            .expect("valid timestamp");
        let first = observation(
            "a",
            SupportStatus::Verified,
            ObservationSource::ProviderApi,
            Confidence::High,
            now,
            None,
        );
        let second = observation(
            "b",
            SupportStatus::Verified,
            ObservationSource::ProviderApi,
            Confidence::High,
            now,
            None,
        );
        let merged = merge_capability_observations(&[second.clone(), first.clone()], now)
            .expect("merge observations");
        assert_eq!(merged.selected(), &first);

        let mut other_route = second;
        other_route.model_route_id = ModelRouteId::from("route-b");
        assert!(merge_capability_observations(&[first, other_route], now).is_err());
        assert!(merge_capability_observations(&[], now).is_err());
    }

    #[test]
    fn expired_disagreement_remains_visible_but_is_not_a_current_conflict() {
        let now = Utc
            .timestamp_opt(10_000, 0)
            .single()
            .expect("valid timestamp");
        let current = observation(
            "current",
            SupportStatus::Verified,
            ObservationSource::ProviderApi,
            Confidence::High,
            now,
            Some(now + chrono::Duration::seconds(100)),
        );
        let expired = observation(
            "expired",
            SupportStatus::Unsupported,
            ObservationSource::CapabilityProbe,
            Confidence::High,
            now - chrono::Duration::seconds(200),
            Some(now - chrono::Duration::seconds(100)),
        );
        let merged = merge_capability_observations(&[expired.clone(), current.clone()], now)
            .expect("merge current and expired evidence");
        assert_eq!(merged.selected(), &current);
        assert_eq!(merged.alternatives(), &[expired]);
        assert!(!merged.has_conflict());
    }

    #[test]
    fn structured_provider_metadata_beats_probe_for_token_limits() {
        let now = Utc
            .timestamp_opt(10_000, 0)
            .single()
            .expect("valid timestamp");
        let provider_metadata = CapabilityObservation {
            id: ObservationId::from("provider-api"),
            model_route_id: ModelRouteId::from("route-a"),
            key: CapabilityKey::ContextWindow,
            value: CapabilityValue::Integer(128_000),
            status: SupportStatus::Verified,
            source: ObservationSource::ProviderApi,
            confidence: Confidence::High,
            observed_at: now,
            expires_at: None,
            evidence_ref: None,
        };
        let probe = CapabilityObservation {
            id: ObservationId::from("probe"),
            model_route_id: ModelRouteId::from("route-a"),
            key: CapabilityKey::ContextWindow,
            value: CapabilityValue::Integer(64_000),
            status: SupportStatus::Inferred,
            source: ObservationSource::CapabilityProbe,
            confidence: Confidence::High,
            observed_at: now,
            expires_at: None,
            evidence_ref: None,
        };

        let merged = merge_capability_observations(&[probe, provider_metadata.clone()], now)
            .expect("merge token-limit observations");
        assert_eq!(merged.selected(), &provider_metadata);
        assert!(merged.has_conflict());
    }

    #[tokio::test]
    async fn public_probe_surface_contains_no_credential_or_provider_text() {
        let adapter = Arc::new(SyntheticAdapter::new(SyntheticBehavior::Error(
            ProbeAdapterError::Authentication,
        )));
        let engine = CapabilityProbeEngine::new();
        let route = ModelRouteId::from("route-a");
        let (_sender, receiver) = watch::channel(false);
        let outcome = engine
            .run(
                adapter,
                &route,
                CapabilityProbeKind::StructuredOutput,
                consent(
                    CONSENT_C,
                    &route,
                    CapabilityProbeKind::StructuredOutput,
                    Duration::from_secs(1),
                ),
                receiver,
            )
            .await;
        assert_eq!(
            outcome,
            ProbeRunOutcome::Failed(ProbeFailure::Authentication)
        );
        let debug = format!("{outcome:?}");
        assert!(!debug.to_ascii_lowercase().contains("authorization"));
        assert!(!debug.to_ascii_lowercase().contains("credential"));
        assert!(!debug.contains("sk-test-secret"));
    }
}
