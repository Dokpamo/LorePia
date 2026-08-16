//! Provider prompt-role, cache-boundary, and token-budget contracts.
//!
//! Prompt resolution remains provider-neutral. This module is the closed
//! adapter boundary that explains how a resolved semantic role or cache
//! directive is represented by a concrete API family. Decisions contain no
//! prompt text, credentials, or provider-controlled scalar values, so they are
//! safe to include in request previews and resolution traces.

use std::{fmt, num::NonZeroU32};

use lorepia_domain::{
    ApiFamily, CacheBoundaryId, CacheDirectiveStatus, CacheMode, CacheRoleFilter, CacheTtl,
    InstructionAuthority, MessageId, MessageRole, PromptBlockId, ProviderMessageRole,
    ResolvedCacheDirective, ResolvedPromptPlan, RoleHint, UnsupportedRolePolicy,
};
use lorepia_orchestration::verify_resolved_prompt_plan;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::parameter_mapping::PromptCacheWireDialect;

pub use lorepia_domain::ProviderPromptContract;

/// Route-specific evidence for a developer-role wire message.
///
/// The Responses family supports the role by default. Generic OpenAI-compatible
/// routes require positive capability evidence, because not every compatible
/// endpoint accepts it. Native non-OpenAI adapters remain closed even if
/// malformed capability metadata claims otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeveloperRoleCapability {
    Supported,
    Unsupported,
    Unknown,
}

/// Closed role names which may appear on a provider request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderWireRole {
    System,
    Developer,
    User,
    Assistant,
    Model,
}

impl ProviderWireRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Developer => "developer",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Model => "model",
        }
    }
}

/// Structural location used by an adapter for a mapped prompt item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPromptPlacement {
    Message,
    SystemInstruction,
}

/// Stable explanation for a role resolution or downgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRoleMappingReason {
    ProviderDefaultResolved,
    DeveloperRoleUnsupportedByFamily,
    DeveloperRoleUnsupportedForRoute,
    DeveloperRoleCapabilityNotVerified,
    UnsupportedRoleMappedByContract,
}

/// Explainable result of mapping one semantic prompt role to a provider wire
/// location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRoleMapping {
    pub requested_role: RoleHint,
    pub effective_role: ProviderMessageRole,
    pub authority: InstructionAuthority,
    pub wire_role: ProviderWireRole,
    pub placement: ProviderPromptPlacement,
    pub reason: Option<ProviderRoleMappingReason>,
}

/// Family-level cache architecture. Exact route capability metadata still
/// decides whether an individual cache mode can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCacheBoundaryArchitecture {
    ProviderManaged,
    InlineBreakpoints,
    ExternalCachedContext,
    Unsupported,
}

/// Concrete cache action which can be compiled for the selected route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCacheBoundaryStrategy {
    ProviderManagedAutomatic,
    AnthropicInlineBreakpoint,
    CachingDisabled,
}

/// Stable warning emitted when a requested cache boundary cannot be
/// represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCacheBoundaryWarning {
    PromptCachingUnavailable,
    ProviderManagedCachingHasNoExplicitBoundary,
    ExplicitCachedContextMustBeCreatedSeparately,
    RequestedCacheModeUnavailable,
    CacheDisableUnavailable,
    CacheBoundaryLimitExceeded,
    CacheBoundaryTargetWasRemoved,
    CacheRoleFilterUnavailable,
    RequestedCacheTtlUnavailable,
}

/// Explainable result of compiling one provider-neutral cache directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum ProviderCacheBoundaryDisposition {
    NoDirective,
    Mapped {
        strategy: ProviderCacheBoundaryStrategy,
    },
    Ignored {
        warning: ProviderCacheBoundaryWarning,
    },
}

/// A resolved cache directive paired with its provider-specific disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCacheBoundaryCompilation {
    pub boundary_id: CacheBoundaryId,
    pub after_block_id: PromptBlockId,
    pub after_message_sequence: Option<u32>,
    pub role_filter: CacheRoleFilter,
    pub ttl: CacheTtl,
    pub mode: CacheMode,
    pub disposition: ProviderCacheBoundaryDisposition,
}

/// One provider-ready prompt message.
///
/// Prompt text is intentionally not serializable and its `Debug`
/// representation is redacted. Only the adapter request builder should read
/// [`Self::content`].
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderPromptMessage {
    sequence: u32,
    block_id: PromptBlockId,
    effective_role: ProviderMessageRole,
    wire_role: ProviderWireRole,
    placement: ProviderPromptPlacement,
    estimated_tokens: u32,
    source_message_ids: Vec<MessageId>,
    content: String,
}

impl ProviderPromptMessage {
    pub const fn sequence(&self) -> u32 {
        self.sequence
    }

    pub const fn effective_role(&self) -> ProviderMessageRole {
        self.effective_role
    }

    pub const fn wire_role(&self) -> ProviderWireRole {
        self.wire_role
    }

    pub const fn placement(&self) -> ProviderPromptPlacement {
        self.placement
    }

    pub const fn estimated_tokens(&self) -> u32 {
        self.estimated_tokens
    }

    pub fn block_id(&self) -> &PromptBlockId {
        &self.block_id
    }

    pub fn source_message_ids(&self) -> &[MessageId] {
        &self.source_message_ids
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

impl fmt::Debug for ProviderPromptMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderPromptMessage")
            .field("sequence", &self.sequence)
            .field("block_id", &self.block_id)
            .field("effective_role", &self.effective_role)
            .field("wire_role", &self.wire_role)
            .field("placement", &self.placement)
            .field("estimated_tokens", &self.estimated_tokens)
            .field("source_message_count", &self.source_message_ids.len())
            .field("content", &"[REDACTED]")
            .finish()
    }
}

/// Log-safe metadata for one provider-ready message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderPromptMessagePreview {
    pub sequence: u32,
    pub block_id: PromptBlockId,
    pub effective_role: ProviderMessageRole,
    pub wire_role: ProviderWireRole,
    pub placement: ProviderPromptPlacement,
    pub estimated_tokens: u32,
}

/// Provider-ready result of compiling a provider-neutral resolved prompt.
///
/// Like [`ProviderPromptMessage`], this type deliberately has no serialization
/// implementation. Its custom `Debug` output delegates only to redacted
/// messages.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderCompiledPromptPlan {
    family: ApiFamily,
    source_plan_hash: String,
    execution_hash: String,
    messages: Vec<ProviderPromptMessage>,
    cache_boundaries: Vec<ProviderCacheBoundaryCompilation>,
}

impl ProviderCompiledPromptPlan {
    pub const fn family(&self) -> ApiFamily {
        self.family
    }

    pub fn source_plan_hash(&self) -> &str {
        &self.source_plan_hash
    }

    /// Canonical SHA-256 identity of the exact provider-side role topology and
    /// cache disposition selected for this neutral plan.
    ///
    /// Prompt content and credentials are not inputs. They remain bound
    /// transitively by `source_plan_hash`.
    pub fn execution_hash(&self) -> &str {
        &self.execution_hash
    }

    pub fn messages(&self) -> &[ProviderPromptMessage] {
        &self.messages
    }

    pub fn cache_boundaries(&self) -> &[ProviderCacheBoundaryCompilation] {
        &self.cache_boundaries
    }

    /// Rejects a stale preview identity before any provider payload is sent.
    ///
    /// `None` preserves the legacy request path while callers migrate to
    /// execution-bound previews. Once supplied, the identity must match
    /// exactly.
    pub fn verify_execution_hash(
        &self,
        expected: Option<&str>,
    ) -> Result<(), ProviderPromptContractError> {
        if expected.is_some_and(|expected| expected != self.execution_hash) {
            return Err(ProviderPromptContractError::ExecutionPlanHashMismatch);
        }
        Ok(())
    }

    pub fn preview(&self) -> ProviderCompiledPromptPreview {
        ProviderCompiledPromptPreview {
            family: self.family,
            source_plan_hash: self.source_plan_hash.clone(),
            execution_hash: self.execution_hash.clone(),
            messages: self
                .messages
                .iter()
                .map(|message| ProviderPromptMessagePreview {
                    sequence: message.sequence,
                    block_id: message.block_id.clone(),
                    effective_role: message.effective_role,
                    wire_role: message.wire_role,
                    placement: message.placement,
                    estimated_tokens: message.estimated_tokens,
                })
                .collect(),
            cache_boundaries: self.cache_boundaries.clone(),
        }
    }
}

impl fmt::Debug for ProviderCompiledPromptPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderCompiledPromptPlan")
            .field("family", &self.family)
            .field("source_plan_hash", &self.source_plan_hash)
            .field("execution_hash", &self.execution_hash)
            .field("messages", &self.messages)
            .field("cache_boundaries", &self.cache_boundaries)
            .finish()
    }
}

/// Serializable preview of a compiled plan with prompt text omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCompiledPromptPreview {
    pub family: ApiFamily,
    pub source_plan_hash: String,
    pub execution_hash: String,
    pub messages: Vec<ProviderPromptMessagePreview>,
    pub cache_boundaries: Vec<ProviderCacheBoundaryCompilation>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderExecutionIdentity<'a> {
    schema_version: u8,
    source_plan_hash: &'a str,
    family: ApiFamily,
    messages: Vec<ProviderExecutionMessageIdentity<'a>>,
    cache_boundaries: &'a [ProviderCacheBoundaryCompilation],
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderExecutionMessageIdentity<'a> {
    sequence: u32,
    block_id: &'a PromptBlockId,
    effective_role: ProviderMessageRole,
    wire_role: ProviderWireRole,
    placement: ProviderPromptPlacement,
}

/// Tokenizer family selected for prompt estimates.
///
/// Tokenization is model-specific even inside one API family. Until an exact
/// tokenizer is registered for a selected route, [`Self::estimate_text`]
/// returns a deliberately conservative UTF-8 byte estimate and marks it as
/// inexact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPromptTokenizer {
    OpenAiModelSpecific,
    AnthropicModelSpecific,
    GeminiModelSpecific,
    OllamaModelSpecific,
}

impl ProviderPromptTokenizer {
    pub fn estimate_text(self, text: &str) -> PromptTokenEstimate {
        PromptTokenEstimate {
            tokens: u64::try_from(text.len()).unwrap_or(u64::MAX),
            exact: false,
        }
    }
}

/// One boundedness-oriented token estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptTokenEstimate {
    pub tokens: u64,
    pub exact: bool,
}

/// Context limit known for the selected model route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "tokens", rename_all = "snake_case")]
pub enum ProviderContextLimit {
    Unknown,
    Observed(NonZeroU32),
}

impl ProviderContextLimit {
    pub const fn tokens(self) -> Option<u32> {
        match self {
            Self::Unknown => None,
            Self::Observed(tokens) => Some(tokens.get()),
        }
    }

    pub fn from_tokens(tokens: Option<u32>) -> Result<Self, ProviderPromptContractError> {
        match tokens {
            None => Ok(Self::Unknown),
            Some(tokens) => NonZeroU32::new(tokens)
                .map(Self::Observed)
                .ok_or(ProviderPromptContractError::ZeroContextLimit),
        }
    }
}

/// Closed provider-adapter metadata used to construct the provider-neutral
/// resolver contract and compile its output for one API family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderPromptAdapterContract {
    family: ApiFamily,
    tokenizer: ProviderPromptTokenizer,
    context_limit: ProviderContextLimit,
    cache_architecture: ProviderCacheBoundaryArchitecture,
}

impl ProviderPromptAdapterContract {
    pub const fn for_family(family: ApiFamily) -> Self {
        let (tokenizer, cache_architecture) = match family {
            ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions => (
                ProviderPromptTokenizer::OpenAiModelSpecific,
                ProviderCacheBoundaryArchitecture::ProviderManaged,
            ),
            ApiFamily::AnthropicMessages => (
                ProviderPromptTokenizer::AnthropicModelSpecific,
                ProviderCacheBoundaryArchitecture::InlineBreakpoints,
            ),
            ApiFamily::GeminiGenerateContent => (
                ProviderPromptTokenizer::GeminiModelSpecific,
                ProviderCacheBoundaryArchitecture::ExternalCachedContext,
            ),
            ApiFamily::OllamaNative => (
                ProviderPromptTokenizer::OllamaModelSpecific,
                ProviderCacheBoundaryArchitecture::Unsupported,
            ),
        };
        Self {
            family,
            tokenizer,
            context_limit: ProviderContextLimit::Unknown,
            cache_architecture,
        }
    }

    pub const fn family(self) -> ApiFamily {
        self.family
    }

    pub const fn tokenizer(self) -> ProviderPromptTokenizer {
        self.tokenizer
    }

    pub const fn context_limit(self) -> ProviderContextLimit {
        self.context_limit
    }

    pub const fn cache_architecture(self) -> ProviderCacheBoundaryArchitecture {
        self.cache_architecture
    }

    pub fn with_context_limit_tokens(
        mut self,
        tokens: Option<u32>,
    ) -> Result<Self, ProviderPromptContractError> {
        self.context_limit = ProviderContextLimit::from_tokens(tokens)?;
        Ok(self)
    }

    /// Produces the exact provider-neutral contract consumed by prompt
    /// resolution. Positive route capability evidence can enable `developer`
    /// only for `OpenAI` wire families; negative evidence overrides their
    /// family default.
    pub fn resolution_contract(
        self,
        developer_capability: DeveloperRoleCapability,
    ) -> ProviderPromptContract {
        let supports_developer = match self.family {
            ApiFamily::OpenAiResponses => {
                developer_capability != DeveloperRoleCapability::Unsupported
            }
            ApiFamily::OpenAiChatCompletions => {
                developer_capability == DeveloperRoleCapability::Supported
            }
            ApiFamily::AnthropicMessages
            | ApiFamily::GeminiGenerateContent
            | ApiFamily::OllamaNative => false,
        };
        let mut supported_roles = vec![
            ProviderMessageRole::System,
            ProviderMessageRole::User,
            ProviderMessageRole::Assistant,
        ];
        if supports_developer {
            supported_roles.insert(1, ProviderMessageRole::Developer);
        }
        let (supports_explicit_cache, max_cache_boundaries) = match self.family {
            ApiFamily::AnthropicMessages => (true, 4),
            ApiFamily::OpenAiResponses
            | ApiFamily::OpenAiChatCompletions
            | ApiFamily::GeminiGenerateContent
            | ApiFamily::OllamaNative => (false, 0),
        };
        ProviderPromptContract {
            supported_roles,
            provider_default_role: ProviderMessageRole::User,
            unsupported_role_policy: UnsupportedRolePolicy::MapDeveloperToSystem,
            supports_explicit_cache,
            max_cache_boundaries,
        }
    }

    /// Maps a semantic role without rendering or retaining prompt text.
    pub fn map_role(
        self,
        requested_role: RoleHint,
        authority: InstructionAuthority,
        developer_capability: DeveloperRoleCapability,
    ) -> Result<ProviderRoleMapping, ProviderPromptContractError> {
        let contract = self.resolution_contract(developer_capability);
        let mut reason = None;
        let resolved_role = if authority == InstructionAuthority::ImportedContent
            && matches!(requested_role, RoleHint::System | RoleHint::Developer)
        {
            reason = Some(ProviderRoleMappingReason::UnsupportedRoleMappedByContract);
            ProviderMessageRole::User
        } else {
            match requested_role {
                RoleHint::System => ProviderMessageRole::System,
                RoleHint::Developer => ProviderMessageRole::Developer,
                RoleHint::User => ProviderMessageRole::User,
                RoleHint::Assistant => ProviderMessageRole::Assistant,
                RoleHint::ProviderDefault => {
                    reason = Some(ProviderRoleMappingReason::ProviderDefaultResolved);
                    contract.provider_default_role
                }
            }
        };
        let effective_role = if contract.supported_roles.contains(&resolved_role) {
            resolved_role
        } else {
            reason = Some(self.unsupported_role_reason(resolved_role, developer_capability));
            match contract.unsupported_role_policy {
                UnsupportedRolePolicy::MapDeveloperToSystem
                    if resolved_role == ProviderMessageRole::Developer =>
                {
                    ProviderMessageRole::System
                }
                UnsupportedRolePolicy::MapSystemToDeveloper
                    if resolved_role == ProviderMessageRole::System =>
                {
                    ProviderMessageRole::Developer
                }
                UnsupportedRolePolicy::UseProviderDefault => contract.provider_default_role,
                UnsupportedRolePolicy::Reject
                | UnsupportedRolePolicy::MapDeveloperToSystem
                | UnsupportedRolePolicy::MapSystemToDeveloper => {
                    return Err(ProviderPromptContractError::UnsupportedRole);
                }
            }
        };
        let (wire_role, placement) = self.wire_mapping(effective_role);
        Ok(ProviderRoleMapping {
            requested_role,
            effective_role,
            authority,
            wire_role,
            placement,
            reason,
        })
    }

    /// Maps an existing persisted chat role through the same path used by
    /// resolved prompt blocks. Persisted roles are supported by every compiled
    /// adapter, so this operation is infallible.
    pub fn map_message_role(self, role: MessageRole) -> ProviderRoleMapping {
        let requested_role = match role {
            MessageRole::System => RoleHint::System,
            MessageRole::User => RoleHint::User,
            MessageRole::Assistant => RoleHint::Assistant,
        };
        let effective_role = match role {
            MessageRole::System => ProviderMessageRole::System,
            MessageRole::User => ProviderMessageRole::User,
            MessageRole::Assistant => ProviderMessageRole::Assistant,
        };
        let (wire_role, placement) = self.wire_mapping(effective_role);
        ProviderRoleMapping {
            requested_role,
            effective_role,
            authority: InstructionAuthority::Conversation,
            wire_role,
            placement,
            reason: None,
        }
    }

    /// Compiles the exact messages and cache directives from a resolved plan.
    ///
    /// The resolver's effective role is checked against this adapter contract
    /// before prompt text is materialized. This prevents a stale or
    /// family-mismatched resolution contract from silently changing authority
    /// at the network boundary.
    pub fn compile_resolved_plan(
        self,
        plan: &ResolvedPromptPlan,
        developer_capability: DeveloperRoleCapability,
        cache_dialect: PromptCacheWireDialect,
    ) -> Result<ProviderCompiledPromptPlan, ProviderPromptContractError> {
        verify_resolved_prompt_plan(plan)
            .map_err(|_| ProviderPromptContractError::InvalidResolvedPlan)?;
        let mut messages = Vec::with_capacity(plan.effective_messages.len());
        for message in &plan.effective_messages {
            let mapping = self.map_role(
                message.requested_role,
                message.authority,
                developer_capability,
            )?;
            if mapping.effective_role != message.effective_role {
                return Err(ProviderPromptContractError::ResolvedRoleContractMismatch);
            }
            messages.push(ProviderPromptMessage {
                sequence: message.sequence,
                block_id: message.block_id.clone(),
                effective_role: message.effective_role,
                wire_role: mapping.wire_role,
                placement: mapping.placement,
                estimated_tokens: message.estimated_tokens,
                source_message_ids: message.source_message_ids.clone(),
                content: message.content.clone(),
            });
        }

        let resolution_contract = self.resolution_contract(developer_capability);
        let mut applied_explicit_boundaries = 0_u32;
        let mut cache_boundaries = Vec::with_capacity(plan.cache_directives.len());
        for directive in &plan.cache_directives {
            let over_limit = resolution_contract.supports_explicit_cache
                && directive.status == CacheDirectiveStatus::Applied
                && directive.mode == CacheMode::Explicit
                && applied_explicit_boundaries >= resolution_contract.max_cache_boundaries;
            let compilation = if over_limit {
                ProviderCacheBoundaryCompilation {
                    boundary_id: directive.boundary_id.clone(),
                    after_block_id: directive.after_block_id.clone(),
                    after_message_sequence: directive.after_message_sequence,
                    role_filter: directive.role_filter,
                    ttl: directive.ttl,
                    mode: directive.mode,
                    disposition: ignored(ProviderCacheBoundaryWarning::CacheBoundaryLimitExceeded),
                }
            } else {
                let compilation = self.compile_cache_directive(directive, cache_dialect)?;
                if directive.status == CacheDirectiveStatus::Applied
                    && directive.mode == CacheMode::Explicit
                    && matches!(
                        compilation.disposition,
                        ProviderCacheBoundaryDisposition::Mapped { .. }
                    )
                {
                    applied_explicit_boundaries = applied_explicit_boundaries.saturating_add(1);
                }
                compilation
            };
            cache_boundaries.push(compilation);
        }

        let execution_hash =
            provider_execution_hash(self.family, &plan.plan_hash, &messages, &cache_boundaries)?;
        Ok(ProviderCompiledPromptPlan {
            family: self.family,
            source_plan_hash: plan.plan_hash.clone(),
            execution_hash,
            messages,
            cache_boundaries,
        })
    }

    /// Compiles a plan against the exact cache dialect retained by the
    /// route-derived `ProviderRequestPlan`. No family default is synthesized
    /// when route capability evidence is absent.
    pub(crate) fn compile_resolved_plan_for_execution(
        self,
        plan: &ResolvedPromptPlan,
        cache_dialect: PromptCacheWireDialect,
    ) -> Result<ProviderCompiledPromptPlan, ProviderPromptContractError> {
        let mut developer_capability = None;
        for message in plan.effective_messages.iter().filter(|message| {
            message.requested_role == RoleHint::Developer
                && message.authority != InstructionAuthority::ImportedContent
        }) {
            let observed = match message.effective_role {
                ProviderMessageRole::Developer => DeveloperRoleCapability::Supported,
                ProviderMessageRole::System => DeveloperRoleCapability::Unsupported,
                ProviderMessageRole::User | ProviderMessageRole::Assistant => {
                    return Err(ProviderPromptContractError::InconsistentDeveloperRoleMapping);
                }
            };
            if developer_capability.is_some_and(|current| current != observed) {
                return Err(ProviderPromptContractError::InconsistentDeveloperRoleMapping);
            }
            developer_capability = Some(observed);
        }
        self.compile_resolved_plan(
            plan,
            developer_capability.unwrap_or(DeveloperRoleCapability::Unknown),
            cache_dialect,
        )
    }

    /// Compiles one cache mode against exact, route-derived cache dialect
    /// metadata.
    ///
    /// Unsupported cache boundaries are represented as ignored warnings. A
    /// family-mismatched dialect is a hard error because it signals corrupted
    /// or incorrectly routed capability metadata.
    pub fn map_cache_boundary(
        self,
        mode: CacheMode,
        dialect: PromptCacheWireDialect,
    ) -> Result<ProviderCacheBoundaryDisposition, ProviderPromptContractError> {
        if !cache_dialect_matches_family(self.family, dialect) {
            return Err(ProviderPromptContractError::CacheDialectFamilyMismatch);
        }
        let disposition = match dialect {
            PromptCacheWireDialect::Unsupported => {
                ignored(ProviderCacheBoundaryWarning::PromptCachingUnavailable)
            }
            PromptCacheWireDialect::OpenAiAutomatic { .. } => match mode {
                CacheMode::Automatic => {
                    mapped(ProviderCacheBoundaryStrategy::ProviderManagedAutomatic)
                }
                CacheMode::Explicit => ignored(
                    ProviderCacheBoundaryWarning::ProviderManagedCachingHasNoExplicitBoundary,
                ),
                CacheMode::Disabled => {
                    ignored(ProviderCacheBoundaryWarning::CacheDisableUnavailable)
                }
            },
            PromptCacheWireDialect::Anthropic {
                supports_automatic,
                supports_explicit_breakpoints,
                ..
            } => match mode {
                CacheMode::Automatic if supports_automatic => {
                    mapped(ProviderCacheBoundaryStrategy::ProviderManagedAutomatic)
                }
                CacheMode::Explicit if supports_explicit_breakpoints => {
                    mapped(ProviderCacheBoundaryStrategy::AnthropicInlineBreakpoint)
                }
                CacheMode::Disabled => mapped(ProviderCacheBoundaryStrategy::CachingDisabled),
                CacheMode::Automatic | CacheMode::Explicit => {
                    ignored(ProviderCacheBoundaryWarning::RequestedCacheModeUnavailable)
                }
            },
            PromptCacheWireDialect::Gemini {
                supports_implicit,
                supports_explicit_context,
            } => match mode {
                CacheMode::Automatic if supports_implicit => {
                    mapped(ProviderCacheBoundaryStrategy::ProviderManagedAutomatic)
                }
                CacheMode::Explicit if supports_explicit_context => ignored(
                    ProviderCacheBoundaryWarning::ExplicitCachedContextMustBeCreatedSeparately,
                ),
                CacheMode::Disabled => {
                    ignored(ProviderCacheBoundaryWarning::CacheDisableUnavailable)
                }
                CacheMode::Automatic | CacheMode::Explicit => {
                    ignored(ProviderCacheBoundaryWarning::RequestedCacheModeUnavailable)
                }
            },
        };
        Ok(disposition)
    }

    /// Retains resolved cache provenance while attaching the adapter action or
    /// warning that the request compiler must apply.
    pub fn compile_cache_directive(
        self,
        directive: &ResolvedCacheDirective,
        dialect: PromptCacheWireDialect,
    ) -> Result<ProviderCacheBoundaryCompilation, ProviderPromptContractError> {
        if !cache_dialect_matches_family(self.family, dialect) {
            return Err(ProviderPromptContractError::CacheDialectFamilyMismatch);
        }
        let disposition = match directive.status {
            CacheDirectiveStatus::Applied
                if directive.role_filter != CacheRoleFilter::All
                    && directive.mode == CacheMode::Explicit =>
            {
                ignored(ProviderCacheBoundaryWarning::CacheRoleFilterUnavailable)
            }
            CacheDirectiveStatus::Applied
                if directive.ttl == CacheTtl::Long
                    && matches!(
                        dialect,
                        PromptCacheWireDialect::Anthropic {
                            supports_one_hour_ttl: false,
                            ..
                        }
                    ) =>
            {
                ignored(ProviderCacheBoundaryWarning::RequestedCacheTtlUnavailable)
            }
            CacheDirectiveStatus::Applied => self.map_cache_boundary(directive.mode, dialect)?,
            CacheDirectiveStatus::IgnoredUnsupported => {
                ignored(ProviderCacheBoundaryWarning::PromptCachingUnavailable)
            }
            CacheDirectiveStatus::IgnoredLimit => {
                ignored(ProviderCacheBoundaryWarning::CacheBoundaryLimitExceeded)
            }
            CacheDirectiveStatus::RemovedWithBlock => {
                ignored(ProviderCacheBoundaryWarning::CacheBoundaryTargetWasRemoved)
            }
        };
        Ok(ProviderCacheBoundaryCompilation {
            boundary_id: directive.boundary_id.clone(),
            after_block_id: directive.after_block_id.clone(),
            after_message_sequence: directive.after_message_sequence,
            role_filter: directive.role_filter,
            ttl: directive.ttl,
            mode: directive.mode,
            disposition,
        })
    }

    fn unsupported_role_reason(
        self,
        role: ProviderMessageRole,
        developer_capability: DeveloperRoleCapability,
    ) -> ProviderRoleMappingReason {
        if role != ProviderMessageRole::Developer {
            return ProviderRoleMappingReason::UnsupportedRoleMappedByContract;
        }
        match (self.family, developer_capability) {
            (
                ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions,
                DeveloperRoleCapability::Unsupported,
            ) => ProviderRoleMappingReason::DeveloperRoleUnsupportedForRoute,
            (ApiFamily::OpenAiChatCompletions, DeveloperRoleCapability::Unknown) => {
                ProviderRoleMappingReason::DeveloperRoleCapabilityNotVerified
            }
            _ => ProviderRoleMappingReason::DeveloperRoleUnsupportedByFamily,
        }
    }

    fn wire_mapping(
        self,
        role: ProviderMessageRole,
    ) -> (ProviderWireRole, ProviderPromptPlacement) {
        match (self.family, role) {
            (
                ApiFamily::AnthropicMessages | ApiFamily::GeminiGenerateContent,
                ProviderMessageRole::System,
            ) => (
                ProviderWireRole::System,
                ProviderPromptPlacement::SystemInstruction,
            ),
            (ApiFamily::GeminiGenerateContent, ProviderMessageRole::Assistant) => {
                (ProviderWireRole::Model, ProviderPromptPlacement::Message)
            }
            (_, ProviderMessageRole::System) => {
                (ProviderWireRole::System, ProviderPromptPlacement::Message)
            }
            (_, ProviderMessageRole::Developer) => (
                ProviderWireRole::Developer,
                ProviderPromptPlacement::Message,
            ),
            (_, ProviderMessageRole::User) => {
                (ProviderWireRole::User, ProviderPromptPlacement::Message)
            }
            (_, ProviderMessageRole::Assistant) => (
                ProviderWireRole::Assistant,
                ProviderPromptPlacement::Message,
            ),
        }
    }
}

/// Stable failures from provider prompt-contract compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPromptContractError {
    ZeroContextLimit,
    UnsupportedRole,
    InvalidResolvedPlan,
    ResolvedRoleContractMismatch,
    InconsistentDeveloperRoleMapping,
    CacheDialectFamilyMismatch,
    ExecutionIdentityEncoding,
    ExecutionPlanHashMismatch,
}

impl fmt::Display for ProviderPromptContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ZeroContextLimit => "provider context limit must be greater than zero",
            Self::UnsupportedRole => {
                "provider prompt role is unsupported by the selected adapter contract"
            }
            Self::InvalidResolvedPlan => {
                "resolved prompt plan failed shape, preview, or hash verification"
            }
            Self::ResolvedRoleContractMismatch => {
                "resolved prompt role does not match the selected adapter contract"
            }
            Self::InconsistentDeveloperRoleMapping => {
                "resolved prompt plan contains inconsistent developer-role decisions"
            }
            Self::CacheDialectFamilyMismatch => {
                "prompt-cache capability dialect does not match the adapter API family"
            }
            Self::ExecutionIdentityEncoding => {
                "provider execution identity could not be encoded canonically"
            }
            Self::ExecutionPlanHashMismatch => {
                "provider execution plan changed after preview; resolve a new preview before sending"
            }
        })
    }
}

impl std::error::Error for ProviderPromptContractError {}

const fn mapped(strategy: ProviderCacheBoundaryStrategy) -> ProviderCacheBoundaryDisposition {
    ProviderCacheBoundaryDisposition::Mapped { strategy }
}

const fn ignored(warning: ProviderCacheBoundaryWarning) -> ProviderCacheBoundaryDisposition {
    ProviderCacheBoundaryDisposition::Ignored { warning }
}

fn provider_execution_hash(
    family: ApiFamily,
    source_plan_hash: &str,
    messages: &[ProviderPromptMessage],
    cache_boundaries: &[ProviderCacheBoundaryCompilation],
) -> Result<String, ProviderPromptContractError> {
    let identity = ProviderExecutionIdentity {
        schema_version: 1,
        source_plan_hash,
        family,
        messages: messages
            .iter()
            .map(|message| ProviderExecutionMessageIdentity {
                sequence: message.sequence,
                block_id: &message.block_id,
                effective_role: message.effective_role,
                wire_role: message.wire_role,
                placement: message.placement,
            })
            .collect(),
        cache_boundaries,
    };
    let canonical_json = serde_json::to_vec(&identity)
        .map_err(|_| ProviderPromptContractError::ExecutionIdentityEncoding)?;
    Ok(format!("{:x}", Sha256::digest(canonical_json)))
}

const fn cache_dialect_matches_family(family: ApiFamily, dialect: PromptCacheWireDialect) -> bool {
    matches!(
        (family, dialect),
        (_, PromptCacheWireDialect::Unsupported)
            | (
                ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions,
                PromptCacheWireDialect::OpenAiAutomatic { .. }
            )
            | (
                ApiFamily::AnthropicMessages,
                PromptCacheWireDialect::Anthropic { .. }
            )
            | (
                ApiFamily::GeminiGenerateContent,
                PromptCacheWireDialect::Gemini { .. }
            )
    )
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use lorepia_domain::{
        AnthropicBlockText, AnthropicContentBlock, AnthropicContentBlockTopology, CacheBoundary,
        CharacterPromptContent, ConversationBranchId, ConversationId, GenerationId,
        GenerationPresetId, GenerationProviderProvenance, GenerationRequest,
        KnowledgeActivationReason, KnowledgeEntryId, KnowledgePlacement, Message, MessageId,
        ModelRouteId, OpaqueReasoningContext, OpaqueReasoningData, OpaqueReasoningState,
        OpenRouterReasoningTopology, ParameterDefaultMode, ParameterId, PresetMetadata,
        PromptConversationMessage, PromptMessageRole, PromptPresetId, PromptResolutionContext,
        PromptResolveRequest, Provenance, ProviderParameterMapping, ProviderParameterTarget,
        SelectedKnowledge, SourceKind, UiParameterLevel, VariableMap,
    };
    use lorepia_orchestration::{default_prompt_preset, resolve_prompt_plan};

    use super::*;
    use crate::parameter_mapping::{
        ParameterEngine, ParameterIssueCode, ParameterSchema, PromptCacheSettings,
        ReasoningSettings, ReasoningWireDialect, TypedParameterSpec,
        validate_and_build_provider_request_plan,
    };

    const PROMPT_CANARY: &str = "prompt-content-canary-never-log";
    const LEGACY_CANARY: &str = "legacy-message-must-not-win";

    fn provenance() -> Provenance {
        Provenance {
            source_kind: SourceKind::UserCreated,
            source_id: None,
            source_hash: None,
            author: None,
            license: None,
            imported_at: None,
        }
    }

    fn resolved_generation_request(
        adapter: ProviderPromptAdapterContract,
        resolver_contract: Option<ProviderPromptContract>,
        cache_boundary_count: u32,
    ) -> GenerationRequest {
        bind_execution_hash(
            adapter,
            build_resolved_generation_request(
                adapter,
                resolver_contract,
                cache_boundary_count,
                false,
                false,
                false,
            ),
        )
    }

    fn resolved_generation_request_with_duplicate_history(
        adapter: ProviderPromptAdapterContract,
    ) -> GenerationRequest {
        bind_execution_hash(
            adapter,
            build_resolved_generation_request(adapter, None, 0, true, false, false),
        )
    }

    fn bind_execution_hash(
        adapter: ProviderPromptAdapterContract,
        mut request: GenerationRequest,
    ) -> GenerationRequest {
        request.provider_execution_plan_hash = request
            .resolved_prompt_plan
            .as_ref()
            .and_then(|plan| {
                adapter
                    .compile_resolved_plan_for_execution(plan, cache_dialect(adapter.family()))
                    .ok()
            })
            .map(|compiled| compiled.execution_hash().to_owned());
        request
    }

    #[allow(clippy::too_many_lines)]
    fn build_resolved_generation_request(
        adapter: ProviderPromptAdapterContract,
        resolver_contract: Option<ProviderPromptContract>,
        cache_boundary_count: u32,
        duplicate_history: bool,
        imported_developer: bool,
        imported_knowledge: bool,
    ) -> GenerationRequest {
        let timestamp = Utc
            .with_ymd_and_hms(2026, 8, 3, 0, 0, 0)
            .single()
            .expect("fixture timestamp");
        let metadata = PresetMetadata {
            description: "provider contract fixture".into(),
            tags: Vec::new(),
            provenance: provenance(),
            created_at: timestamp,
            updated_at: timestamp,
            local_override_of: None,
        };
        let mut preset = default_prompt_preset(
            PromptPresetId::from("provider-contract"),
            "Fixture",
            metadata,
        );
        preset.blocks[0].role_hint = RoleHint::Developer;
        if imported_knowledge {
            preset.blocks[0].kind = lorepia_domain::PromptBlockKind::WorldKnowledge;
            preset.blocks[0].role_hint = RoleHint::System;
            preset.blocks[0].source = lorepia_domain::BlockSource::SelectedKnowledge;
            preset.blocks[0].placement_zone = lorepia_domain::PlacementZone::RetrievedContext;
        } else if imported_developer {
            preset.blocks[0].authority = InstructionAuthority::ImportedContent;
        }
        if duplicate_history {
            let mut duplicate = preset.blocks[1].clone();
            duplicate.id = PromptBlockId::from("provider-contract.duplicate-history");
            duplicate.name = "Duplicate history fixture".into();
            preset.blocks.insert(2, duplicate);
        }
        let cache_target = preset.blocks[0].id.clone();
        preset.cache_boundaries = (0..cache_boundary_count)
            .map(|index| CacheBoundary {
                id: CacheBoundaryId::from(format!("cache-{index}")),
                after_block_id: cache_target.clone(),
                role_filter: CacheRoleFilter::All,
                ttl: CacheTtl::Short,
                mode: CacheMode::Explicit,
            })
            .collect();

        let conversation_id = ConversationId("provider-contract-conversation".into());
        let branch_id = ConversationBranchId("provider-contract-branch".into());
        let latest_user_message_id = MessageId("latest-user".into());
        let request = PromptResolveRequest {
            preset,
            context: PromptResolutionContext {
                conversation_id: conversation_id.clone(),
                branch_id: branch_id.clone(),
                character: CharacterPromptContent {
                    character_id: "fixture-character".into(),
                    name: "Ari".into(),
                    aliases: Vec::new(),
                    description: PROMPT_CANARY.into(),
                    personality: String::new(),
                    scenario: String::new(),
                    first_message: String::new(),
                    dialogue_examples: Vec::new(),
                    system_instruction: String::new(),
                    post_history_instruction: String::new(),
                    alternate_greetings: Vec::new(),
                    knowledge_book_ids: Vec::new(),
                    asset_ids: Vec::new(),
                },
                persona: None,
                user_name: "Sam".into(),
                messages: vec![
                    PromptConversationMessage {
                        id: MessageId("older-user".into()),
                        branch_id: branch_id.clone(),
                        role: PromptMessageRole::User,
                        content: "hello".into(),
                        turn_index: 1,
                    },
                    PromptConversationMessage {
                        id: MessageId("older-assistant".into()),
                        branch_id: branch_id.clone(),
                        role: PromptMessageRole::Assistant,
                        content: "hello back".into(),
                        turn_index: 1,
                    },
                    PromptConversationMessage {
                        id: latest_user_message_id.clone(),
                        branch_id,
                        role: PromptMessageRole::User,
                        content: "continue".into(),
                        turn_index: 2,
                    },
                ],
                latest_user_message_id,
                selected_knowledge: imported_knowledge
                    .then(|| SelectedKnowledge {
                        entry_id: KnowledgeEntryId::from("provider-contract.imported-knowledge"),
                        content: PROMPT_CANARY.to_owned(),
                        placement: KnowledgePlacement::RetrievedContext,
                        priority: 1,
                        evidence: vec![KnowledgeActivationReason::Always],
                        provenance: Provenance {
                            source_kind: SourceKind::ImportedPackage,
                            source_id: Some("dev.lorepia.provider-contract".to_owned()),
                            source_hash: Some("ab".repeat(32)),
                            author: Some("Untrusted package".to_owned()),
                            license: Some("MIT".to_owned()),
                            imported_at: None,
                        },
                    })
                    .into_iter()
                    .collect(),
                selected_memory: Vec::new(),
                summary_boundaries: Vec::new(),
                conversation_summary: None,
                author_note: None,
                group_context: None,
                variables: VariableMap::default(),
                slots: Vec::new(),
                current_date: "2026-08-03".into(),
                current_time: "12:00".into(),
                supported_capabilities: Vec::new(),
                session_seed: Some(42),
                context_snapshot: None,
            },
            provider: resolver_contract
                .unwrap_or_else(|| adapter.resolution_contract(DeveloperRoleCapability::Unknown)),
            generation_preset_id: None,
            max_context_tokens: 4_096,
            reserved_output_tokens: 128,
        };
        let plan = resolve_prompt_plan(&request).expect("valid provider contract fixture");
        GenerationRequest {
            generation_id: GenerationId::new(),
            conversation_id: conversation_id.clone(),
            model: "fixture-model".into(),
            messages: vec![Message::user(conversation_id, LEGACY_CANARY)],
            resolved_prompt_plan: Some(plan),
            provider_execution_plan_hash: None,
            temperature: None,
            max_output_tokens: Some(128),
            provider_provenance: None,
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        }
    }

    fn with_openrouter_opaque_context(mut request: GenerationRequest) -> GenerationRequest {
        let route_id = ModelRouteId::from("openrouter-route");
        let preset_id = GenerationPresetId::from("generation-preset");
        request.provider_provenance = Some(GenerationProviderProvenance {
            api_family: ApiFamily::OpenAiChatCompletions,
            model_route_id: route_id.clone(),
            generation_preset_id: preset_id.clone(),
        });
        request.preserve_opaque_reasoning_state = true;
        request.opaque_reasoning_context = vec![OpaqueReasoningContext {
            source_message_id: MessageId("older-assistant".into()),
            api_family: ApiFamily::OpenAiChatCompletions,
            model: request.model.clone(),
            model_route_id: route_id,
            generation_preset_id: preset_id,
            state: OpaqueReasoningState::OpenRouterReasoning {
                topology: OpenRouterReasoningTopology::new(Some("private reasoning".into()), None)
                    .expect("OpenRouter topology"),
            },
        }];
        request
    }

    fn with_anthropic_opaque_context(mut request: GenerationRequest) -> GenerationRequest {
        let route_id = ModelRouteId::from("anthropic-route");
        let preset_id = GenerationPresetId::from("generation-preset");
        request.provider_provenance = Some(GenerationProviderProvenance {
            api_family: ApiFamily::AnthropicMessages,
            model_route_id: route_id.clone(),
            generation_preset_id: preset_id.clone(),
        });
        request.preserve_opaque_reasoning_state = true;
        request.opaque_reasoning_context = vec![OpaqueReasoningContext {
            source_message_id: MessageId("older-assistant".into()),
            api_family: ApiFamily::AnthropicMessages,
            model: request.model.clone(),
            model_route_id: route_id,
            generation_preset_id: preset_id,
            state: OpaqueReasoningState::AnthropicMessages {
                content_blocks: AnthropicContentBlockTopology::new(vec![
                    AnthropicContentBlock::Thinking {
                        thinking: AnthropicBlockText::parse("private thinking").expect("thinking"),
                        signature: OpaqueReasoningData::parse("private signature")
                            .expect("signature"),
                    },
                    AnthropicContentBlock::Text {
                        text: AnthropicBlockText::parse("hello back").expect("text"),
                    },
                ])
                .expect("Anthropic topology"),
            },
        }];
        request
    }

    const fn cache_dialect(family: ApiFamily) -> PromptCacheWireDialect {
        match family {
            ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions => {
                PromptCacheWireDialect::OpenAiAutomatic {
                    supports_24_hour_retention: false,
                }
            }
            ApiFamily::AnthropicMessages => PromptCacheWireDialect::Anthropic {
                supports_automatic: false,
                supports_explicit_breakpoints: true,
                supports_one_hour_ttl: true,
            },
            ApiFamily::GeminiGenerateContent => PromptCacheWireDialect::Gemini {
                supports_implicit: true,
                supports_explicit_context: true,
            },
            ApiFamily::OllamaNative => PromptCacheWireDialect::Unsupported,
        }
    }

    fn assert_payload_uses_resolved_plan(payload: &serde_json::Value) {
        let encoded = serde_json::to_string(payload).expect("payload JSON");
        assert!(encoded.contains(PROMPT_CANARY));
        assert!(!encoded.contains(LEGACY_CANARY));
    }

    fn assert_loggable_compilation_redacts_prompt(
        adapter: ProviderPromptAdapterContract,
        request: &GenerationRequest,
    ) {
        let compiled = adapter
            .compile_resolved_plan_for_execution(
                request
                    .resolved_prompt_plan
                    .as_ref()
                    .expect("resolved plan"),
                cache_dialect(adapter.family()),
            )
            .expect("compiled plan");
        assert!(
            compiled
                .messages()
                .iter()
                .any(|message| message.content().contains(PROMPT_CANARY))
        );
        let debug = format!("{compiled:?}");
        let preview = serde_json::to_string(&compiled.preview()).expect("loggable preview");
        let request_debug = format!("{request:?}");
        assert!(!debug.contains(PROMPT_CANARY));
        assert!(!preview.contains(PROMPT_CANARY));
        assert!(!request_debug.contains(PROMPT_CANARY));
        assert!(!request_debug.contains(LEGACY_CANARY));
    }

    fn inherited_number_spec(field: &str) -> TypedParameterSpec {
        TypedParameterSpec {
            id: ParameterId::from("temperature"),
            label_key: "parameter.temperature".into(),
            description_key: None,
            schema: ParameterSchema::Number {
                minimum: Some(0.0),
                maximum: Some(2.0),
                step: Some(0.1),
                choices: Vec::new(),
            },
            default_mode: ParameterDefaultMode::ProviderDefault,
            visibility: None,
            rules: Vec::new(),
            provider_mapping: ProviderParameterMapping {
                target: ProviderParameterTarget::RequestBody,
                field_name: field.into(),
            },
            level: UiParameterLevel::Basic,
        }
    }

    #[test]
    fn every_compiled_adapter_exposes_one_shared_prompt_contract() {
        let contracts = [
            crate::openai_responses::prompt_contract(),
            crate::openai_compatible::prompt_contract(),
            crate::anthropic_messages::prompt_contract(),
            crate::gemini_generate_content::prompt_contract(),
            crate::ollama_native::prompt_contract(),
        ];
        let expected = [
            ApiFamily::OpenAiResponses,
            ApiFamily::OpenAiChatCompletions,
            ApiFamily::AnthropicMessages,
            ApiFamily::GeminiGenerateContent,
            ApiFamily::OllamaNative,
        ];
        for family in expected {
            assert_eq!(
                contracts
                    .iter()
                    .filter(|contract| contract.family() == family)
                    .count(),
                1,
                "{family:?} must have exactly one adapter prompt contract"
            );
        }

        for contract in contracts {
            for role in [
                RoleHint::System,
                RoleHint::Developer,
                RoleHint::User,
                RoleHint::Assistant,
                RoleHint::ProviderDefault,
            ] {
                let mapping = contract
                    .map_role(
                        role,
                        InstructionAuthority::Creator,
                        DeveloperRoleCapability::Unknown,
                    )
                    .expect("built-in adapter contract maps every role");
                assert!(
                    mapping.effective_role != ProviderMessageRole::Developer
                        || mapping.wire_role == ProviderWireRole::Developer
                );
            }
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn resolved_plan_materializes_roles_and_cache_in_every_adapter() {
        let responses = crate::openai_responses::prompt_contract();
        let responses_request = resolved_generation_request(responses, None, 1);
        assert_loggable_compilation_redacts_prompt(responses, &responses_request);
        let responses_payload = crate::openai_responses::resolved_prompt_payload_for_contract_test(
            responses_request,
            cache_dialect(ApiFamily::OpenAiResponses),
        )
        .expect("Responses payload");
        assert_payload_uses_resolved_plan(&responses_payload);
        assert!(
            responses_payload["input"]
                .as_array()
                .is_some_and(|messages| {
                    messages
                        .iter()
                        .any(|message| message["role"] == "developer")
                        && messages
                            .iter()
                            .any(|message| message["role"] == "assistant")
                })
        );

        let compatible = crate::openai_compatible::prompt_contract();
        let compatible_request = resolved_generation_request(compatible, None, 1);
        assert_loggable_compilation_redacts_prompt(compatible, &compatible_request);
        let compatible_payload =
            crate::openai_compatible::resolved_prompt_payload_for_contract_test(
                compatible_request,
                cache_dialect(ApiFamily::OpenAiChatCompletions),
            )
            .expect("OpenAI-compatible payload");
        assert_payload_uses_resolved_plan(&compatible_payload);
        assert!(
            compatible_payload["messages"]
                .as_array()
                .is_some_and(|messages| {
                    messages.iter().any(|message| message["role"] == "system")
                        && messages
                            .iter()
                            .any(|message| message["role"] == "assistant")
                })
        );

        let anthropic = crate::anthropic_messages::prompt_contract();
        let anthropic_request = resolved_generation_request(anthropic, None, 1);
        assert_loggable_compilation_redacts_prompt(anthropic, &anthropic_request);
        let anthropic_payload =
            crate::anthropic_messages::resolved_prompt_payload_for_contract_test(
                anthropic_request,
                cache_dialect(ApiFamily::AnthropicMessages),
            )
            .expect("Anthropic payload");
        assert_payload_uses_resolved_plan(&anthropic_payload);
        assert_eq!(
            anthropic_payload["system"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert!(
            anthropic_payload["messages"]
                .as_array()
                .is_some_and(|messages| {
                    messages.iter().any(|message| message["role"] == "user")
                        && messages
                            .iter()
                            .any(|message| message["role"] == "assistant")
                })
        );

        let gemini = crate::gemini_generate_content::prompt_contract();
        let gemini_request = resolved_generation_request(gemini, None, 1);
        assert_loggable_compilation_redacts_prompt(gemini, &gemini_request);
        let gemini_payload =
            crate::gemini_generate_content::resolved_prompt_payload_for_contract_test(
                gemini_request,
                cache_dialect(ApiFamily::GeminiGenerateContent),
            )
            .expect("Gemini payload");
        assert_payload_uses_resolved_plan(&gemini_payload);
        assert!(
            gemini_payload["contents"]
                .as_array()
                .is_some_and(|contents| {
                    contents.iter().any(|content| content["role"] == "user")
                        && contents.iter().any(|content| content["role"] == "model")
                })
        );
        assert!(
            gemini_payload["systemInstruction"]["parts"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["text"] == PROMPT_CANARY))
        );

        let ollama = crate::ollama_native::prompt_contract();
        let ollama_request = resolved_generation_request(ollama, None, 1);
        assert_loggable_compilation_redacts_prompt(ollama, &ollama_request);
        let ollama_payload = crate::ollama_native::resolved_prompt_payload_for_contract_test(
            ollama_request,
            cache_dialect(ApiFamily::OllamaNative),
        )
        .expect("Ollama payload");
        assert_payload_uses_resolved_plan(&ollama_payload);
        assert!(
            ollama_payload["messages"]
                .as_array()
                .is_some_and(|messages| {
                    messages.iter().any(|message| message["role"] == "system")
                        && messages
                            .iter()
                            .any(|message| message["role"] == "assistant")
                })
        );
    }

    #[test]
    fn resolved_role_contract_mismatch_fails_closed() {
        let anthropic = crate::anthropic_messages::prompt_contract();
        let mut incompatible_contract =
            anthropic.resolution_contract(DeveloperRoleCapability::Unknown);
        incompatible_contract
            .supported_roles
            .insert(1, ProviderMessageRole::Developer);
        let request = resolved_generation_request(anthropic, Some(incompatible_contract), 0);
        let error = anthropic
            .compile_resolved_plan_for_execution(
                request
                    .resolved_prompt_plan
                    .as_ref()
                    .expect("resolved plan"),
                cache_dialect(ApiFamily::AnthropicMessages),
            )
            .expect_err("native Anthropic adapter must reject a developer wire role");
        assert_eq!(
            error,
            ProviderPromptContractError::ResolvedRoleContractMismatch
        );
    }

    #[test]
    fn execution_compilation_accepts_imported_developer_downgrade_for_every_family() {
        for family in [
            ApiFamily::OpenAiResponses,
            ApiFamily::OpenAiChatCompletions,
            ApiFamily::AnthropicMessages,
            ApiFamily::GeminiGenerateContent,
            ApiFamily::OllamaNative,
        ] {
            let adapter = ProviderPromptAdapterContract::for_family(family);
            let request = build_resolved_generation_request(adapter, None, 0, false, true, false);
            let plan = request
                .resolved_prompt_plan
                .as_ref()
                .expect("resolved plan");
            let imported_developer = plan
                .effective_messages
                .iter()
                .find(|message| message.requested_role == RoleHint::Developer)
                .expect("imported developer message");
            assert_eq!(
                imported_developer.authority,
                InstructionAuthority::ImportedContent
            );
            assert_eq!(imported_developer.effective_role, ProviderMessageRole::User);

            let compiled = adapter
                .compile_resolved_plan_for_execution(plan, cache_dialect(family))
                .unwrap_or_else(|error| {
                    panic!("{family:?} rejected a valid imported downgrade: {error}")
                });
            let compiled_message = compiled
                .messages()
                .iter()
                .find(|message| message.block_id() == &imported_developer.block_id)
                .expect("compiled imported developer message");
            assert_eq!(compiled_message.effective_role(), ProviderMessageRole::User);
            assert_eq!(compiled_message.wire_role(), ProviderWireRole::User);
        }
    }

    #[test]
    fn imported_knowledge_cannot_inherit_creator_system_authority_for_any_family() {
        for family in [
            ApiFamily::OpenAiResponses,
            ApiFamily::OpenAiChatCompletions,
            ApiFamily::AnthropicMessages,
            ApiFamily::GeminiGenerateContent,
            ApiFamily::OllamaNative,
        ] {
            let adapter = ProviderPromptAdapterContract::for_family(family);
            let request = build_resolved_generation_request(adapter, None, 0, false, false, true);
            let plan = request
                .resolved_prompt_plan
                .as_ref()
                .expect("resolved plan");
            let knowledge = plan
                .effective_messages
                .iter()
                .find(|message| message.content == PROMPT_CANARY)
                .expect("imported knowledge message");
            assert_eq!(knowledge.requested_role, RoleHint::System);
            assert_eq!(knowledge.authority, InstructionAuthority::ImportedContent);
            assert_eq!(knowledge.effective_role, ProviderMessageRole::User);

            let compiled = adapter
                .compile_resolved_plan_for_execution(plan, cache_dialect(family))
                .unwrap_or_else(|error| {
                    panic!("{family:?} rejected downgraded imported knowledge: {error}")
                });
            let compiled_knowledge = compiled
                .messages()
                .iter()
                .find(|message| message.block_id() == &knowledge.block_id)
                .expect("compiled imported knowledge message");
            assert_eq!(
                compiled_knowledge.effective_role(),
                ProviderMessageRole::User
            );
            assert_eq!(compiled_knowledge.wire_role(), ProviderWireRole::User);
        }
    }

    #[test]
    fn opaque_topology_cannot_be_replayed_by_two_resolved_messages() {
        let compatible = crate::openai_compatible::prompt_contract();
        let compatible_request = with_openrouter_opaque_context(
            resolved_generation_request_with_duplicate_history(compatible),
        );
        let compatible_error = crate::openai_compatible::resolved_prompt_payload_for_contract_test(
            compatible_request,
            cache_dialect(ApiFamily::OpenAiChatCompletions),
        )
        .expect_err("one OpenRouter topology cannot be replayed twice");
        assert!(
            compatible_error
                .message
                .contains("reused by multiple resolved prompt messages")
        );

        let anthropic = crate::anthropic_messages::prompt_contract();
        let anthropic_request = with_anthropic_opaque_context(
            resolved_generation_request_with_duplicate_history(anthropic),
        );
        let anthropic_error = crate::anthropic_messages::resolved_prompt_payload_for_contract_test(
            anthropic_request,
            cache_dialect(ApiFamily::AnthropicMessages),
        )
        .expect_err("one Anthropic topology cannot be replayed twice");
        assert!(
            anthropic_error
                .message
                .contains("reused by multiple resolved prompt messages")
        );
    }

    #[test]
    fn explicit_cache_limit_is_visible_as_a_warning() {
        let anthropic = crate::anthropic_messages::prompt_contract();
        let request = resolved_generation_request(anthropic, None, 5);
        let compiled = anthropic
            .compile_resolved_plan_for_execution(
                request
                    .resolved_prompt_plan
                    .as_ref()
                    .expect("resolved plan"),
                cache_dialect(ApiFamily::AnthropicMessages),
            )
            .expect("bounded Anthropic cache compilation");
        assert_eq!(
            compiled
                .cache_boundaries()
                .iter()
                .filter(|boundary| matches!(
                    boundary.disposition,
                    ProviderCacheBoundaryDisposition::Mapped {
                        strategy: ProviderCacheBoundaryStrategy::AnthropicInlineBreakpoint,
                    }
                ))
                .count(),
            4
        );
        assert_eq!(
            compiled
                .cache_boundaries()
                .iter()
                .filter(|boundary| {
                    boundary.disposition
                        == ignored(ProviderCacheBoundaryWarning::CacheBoundaryLimitExceeded)
                })
                .count(),
            1
        );
    }

    #[test]
    fn execution_hash_binds_exact_cache_disposition_without_prompt_text() {
        let anthropic = crate::anthropic_messages::prompt_contract();
        let request = resolved_generation_request(anthropic, None, 1);
        let plan = request
            .resolved_prompt_plan
            .as_ref()
            .expect("resolved plan");
        let supported = anthropic
            .compile_resolved_plan(
                plan,
                DeveloperRoleCapability::Unsupported,
                PromptCacheWireDialect::Anthropic {
                    supports_automatic: false,
                    supports_explicit_breakpoints: true,
                    supports_one_hour_ttl: true,
                },
            )
            .expect("supported cache plan");
        let repeated = anthropic
            .compile_resolved_plan(
                plan,
                DeveloperRoleCapability::Unsupported,
                PromptCacheWireDialect::Anthropic {
                    supports_automatic: false,
                    supports_explicit_breakpoints: true,
                    supports_one_hour_ttl: true,
                },
            )
            .expect("deterministic cache plan");
        let unsupported = anthropic
            .compile_resolved_plan(
                plan,
                DeveloperRoleCapability::Unsupported,
                PromptCacheWireDialect::Anthropic {
                    supports_automatic: false,
                    supports_explicit_breakpoints: false,
                    supports_one_hour_ttl: true,
                },
            )
            .expect("unsupported cache capability is a warning");

        assert_eq!(supported.source_plan_hash(), unsupported.source_plan_hash());
        assert_eq!(supported.execution_hash(), repeated.execution_hash());
        assert_ne!(supported.execution_hash(), unsupported.execution_hash());
        assert_eq!(supported.execution_hash().len(), 64);
        assert!(
            supported
                .execution_hash()
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        let preview = serde_json::to_string(&supported.preview()).expect("preview");
        assert!(preview.contains(supported.execution_hash()));
        assert!(!preview.contains(PROMPT_CANARY));
    }

    #[test]
    fn every_adapter_rejects_a_stale_execution_hash() {
        macro_rules! assert_stale_hash_rejected {
            ($family:expr, $payload:path) => {{
                let adapter = ProviderPromptAdapterContract::for_family($family);
                let mut request = resolved_generation_request(adapter, None, 1);
                request.provider_execution_plan_hash = Some("0".repeat(64));
                let error = $payload(request, cache_dialect($family))
                    .expect_err("stale execution identity must fail before dispatch");
                assert!(
                    error.message.contains("changed after preview"),
                    "{}",
                    error.message
                );
            }};
        }

        assert_stale_hash_rejected!(
            ApiFamily::OpenAiResponses,
            crate::openai_responses::resolved_prompt_payload_for_contract_test
        );
        assert_stale_hash_rejected!(
            ApiFamily::OpenAiChatCompletions,
            crate::openai_compatible::resolved_prompt_payload_for_contract_test
        );
        assert_stale_hash_rejected!(
            ApiFamily::AnthropicMessages,
            crate::anthropic_messages::resolved_prompt_payload_for_contract_test
        );
        assert_stale_hash_rejected!(
            ApiFamily::GeminiGenerateContent,
            crate::gemini_generate_content::resolved_prompt_payload_for_contract_test
        );
        assert_stale_hash_rejected!(
            ApiFamily::OllamaNative,
            crate::ollama_native::resolved_prompt_payload_for_contract_test
        );
    }

    #[test]
    fn generation_parameter_defaults_are_omitted_and_unknown_wires_rejected() {
        let family_fields = [
            (ApiFamily::OpenAiResponses, "temperature"),
            (ApiFamily::OpenAiChatCompletions, "temperature"),
            (ApiFamily::AnthropicMessages, "temperature"),
            (
                ApiFamily::GeminiGenerateContent,
                "generationConfig.temperature",
            ),
            (ApiFamily::OllamaNative, "options.temperature"),
        ];
        for (family, field) in family_fields {
            let engine =
                ParameterEngine::new(vec![inherited_number_spec(field)]).expect("valid schema");
            let validated = engine
                .validate_for_request(&[])
                .expect("provider-default value");
            assert!(validated.applied().is_empty());
            assert_eq!(
                validated.omitted_provider_defaults(),
                &[ParameterId::from("temperature")]
            );
            let plan = validate_and_build_provider_request_plan(
                &engine,
                family,
                &[],
                &ReasoningSettings::default(),
                &ReasoningWireDialect::Unsupported,
                &PromptCacheSettings::default(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect("known inherited field is omitted");
            assert!(plan.body_patches().is_empty());

            let unsupported = ParameterEngine::new(vec![inherited_number_spec("api_key")])
                .expect("shape is valid before family binding");
            let error = validate_and_build_provider_request_plan(
                &unsupported,
                family,
                &[],
                &ReasoningSettings::default(),
                &ReasoningWireDialect::Unsupported,
                &PromptCacheSettings::default(),
                PromptCacheWireDialect::Unsupported,
            )
            .expect_err("unsupported mappings fail closed even when inherited");
            assert_eq!(error.issues[0].code, ParameterIssueCode::UnsupportedMapping);
        }
    }

    #[test]
    fn resolver_contracts_match_family_and_route_capabilities() {
        let responses = ProviderPromptAdapterContract::for_family(ApiFamily::OpenAiResponses)
            .resolution_contract(DeveloperRoleCapability::Unknown);
        assert!(
            responses
                .supported_roles
                .contains(&ProviderMessageRole::Developer)
        );
        assert_eq!(responses.provider_default_role, ProviderMessageRole::User);
        assert!(!responses.supports_explicit_cache);

        let generic = ProviderPromptAdapterContract::for_family(ApiFamily::OpenAiChatCompletions)
            .resolution_contract(DeveloperRoleCapability::Unknown);
        assert!(
            !generic
                .supported_roles
                .contains(&ProviderMessageRole::Developer)
        );

        let anthropic = ProviderPromptAdapterContract::for_family(ApiFamily::AnthropicMessages)
            .resolution_contract(DeveloperRoleCapability::Unknown);
        assert!(anthropic.supports_explicit_cache);
        assert_eq!(anthropic.max_cache_boundaries, 4);
    }

    #[test]
    fn unsupported_developer_roles_downgrade_with_a_stable_reason() {
        let anthropic = ProviderPromptAdapterContract::for_family(ApiFamily::AnthropicMessages)
            .map_role(
                RoleHint::Developer,
                InstructionAuthority::Creator,
                DeveloperRoleCapability::Supported,
            )
            .expect("Anthropic contract maps developer to system");
        assert_eq!(anthropic.wire_role, ProviderWireRole::System);
        assert_eq!(
            anthropic.reason,
            Some(ProviderRoleMappingReason::DeveloperRoleUnsupportedByFamily)
        );
        assert_eq!(
            anthropic.placement,
            ProviderPromptPlacement::SystemInstruction
        );

        let unverified_generic =
            ProviderPromptAdapterContract::for_family(ApiFamily::OpenAiChatCompletions)
                .map_role(
                    RoleHint::Developer,
                    InstructionAuthority::Creator,
                    DeveloperRoleCapability::Unknown,
                )
                .expect("generic OpenAI contract maps developer to system");
        assert_eq!(unverified_generic.wire_role, ProviderWireRole::System);
        assert_eq!(
            unverified_generic.reason,
            Some(ProviderRoleMappingReason::DeveloperRoleCapabilityNotVerified)
        );

        let responses = crate::openai_responses::prompt_contract();
        let negative_route_contract =
            responses.resolution_contract(DeveloperRoleCapability::Unsupported);
        let request = resolved_generation_request(responses, Some(negative_route_contract), 0);
        let compiled = responses
            .compile_resolved_plan_for_execution(
                request
                    .resolved_prompt_plan
                    .as_ref()
                    .expect("resolved plan"),
                cache_dialect(ApiFamily::OpenAiResponses),
            )
            .expect("hash-bound negative route evidence is preserved");
        assert!(compiled.messages().iter().any(|message| {
            message.content().contains(PROMPT_CANARY)
                && message.wire_role() == ProviderWireRole::System
        }));

        let compatible = crate::openai_compatible::prompt_contract();
        let positive_route_contract =
            compatible.resolution_contract(DeveloperRoleCapability::Supported);
        let request = resolved_generation_request(compatible, Some(positive_route_contract), 0);
        let compiled = compatible
            .compile_resolved_plan_for_execution(
                request
                    .resolved_prompt_plan
                    .as_ref()
                    .expect("resolved plan"),
                cache_dialect(ApiFamily::OpenAiChatCompletions),
            )
            .expect("hash-bound positive compatible-route evidence is preserved");
        assert!(compiled.messages().iter().any(|message| {
            message.content().contains(PROMPT_CANARY)
                && message.wire_role() == ProviderWireRole::Developer
        }));
    }

    #[test]
    fn provider_default_is_deterministic_and_explainable() {
        let mapping = ProviderPromptAdapterContract::for_family(ApiFamily::OpenAiChatCompletions)
            .map_role(
                RoleHint::ProviderDefault,
                InstructionAuthority::ImportedContent,
                DeveloperRoleCapability::Unknown,
            )
            .expect("provider-default role is supported");
        assert_eq!(mapping.effective_role, ProviderMessageRole::User);
        assert_eq!(mapping.wire_role, ProviderWireRole::User);
        assert_eq!(
            mapping.reason,
            Some(ProviderRoleMappingReason::ProviderDefaultResolved)
        );
    }

    #[test]
    fn imported_content_privileged_roles_are_downgraded_at_the_wire_boundary() {
        let adapter = ProviderPromptAdapterContract::for_family(ApiFamily::OpenAiResponses);
        for requested_role in [RoleHint::System, RoleHint::Developer] {
            let mapping = adapter
                .map_role(
                    requested_role,
                    InstructionAuthority::ImportedContent,
                    DeveloperRoleCapability::Supported,
                )
                .expect("imported content is representable as user data");
            assert_eq!(mapping.requested_role, requested_role);
            assert_eq!(mapping.effective_role, ProviderMessageRole::User);
            assert_eq!(mapping.wire_role, ProviderWireRole::User);
        }
    }

    #[test]
    fn cache_boundaries_map_or_return_an_explainable_warning() {
        let anthropic = ProviderPromptAdapterContract::for_family(ApiFamily::AnthropicMessages);
        assert_eq!(
            anthropic
                .map_cache_boundary(
                    CacheMode::Explicit,
                    PromptCacheWireDialect::Anthropic {
                        supports_automatic: false,
                        supports_explicit_breakpoints: true,
                        supports_one_hour_ttl: true,
                    },
                )
                .expect("matching cache dialect"),
            mapped(ProviderCacheBoundaryStrategy::AnthropicInlineBreakpoint)
        );

        let openai = ProviderPromptAdapterContract::for_family(ApiFamily::OpenAiResponses);
        assert_eq!(
            openai
                .map_cache_boundary(
                    CacheMode::Explicit,
                    PromptCacheWireDialect::OpenAiAutomatic {
                        supports_24_hour_retention: false,
                    },
                )
                .expect("matching cache dialect"),
            ignored(ProviderCacheBoundaryWarning::ProviderManagedCachingHasNoExplicitBoundary)
        );

        let gemini = ProviderPromptAdapterContract::for_family(ApiFamily::GeminiGenerateContent);
        assert_eq!(
            gemini
                .map_cache_boundary(
                    CacheMode::Explicit,
                    PromptCacheWireDialect::Gemini {
                        supports_implicit: true,
                        supports_explicit_context: true,
                    },
                )
                .expect("matching cache dialect"),
            ignored(ProviderCacheBoundaryWarning::ExplicitCachedContextMustBeCreatedSeparately)
        );
    }

    #[test]
    fn context_limit_and_token_estimate_never_claim_unavailable_precision() {
        let contract = ProviderPromptAdapterContract::for_family(ApiFamily::OllamaNative)
            .with_context_limit_tokens(Some(8_192))
            .expect("non-zero context limit");
        assert_eq!(contract.context_limit().tokens(), Some(8_192));
        assert_eq!(
            contract.tokenizer().estimate_text("가"),
            PromptTokenEstimate {
                tokens: 3,
                exact: false,
            }
        );
        assert_eq!(
            ProviderPromptAdapterContract::for_family(ApiFamily::OllamaNative)
                .with_context_limit_tokens(Some(0)),
            Err(ProviderPromptContractError::ZeroContextLimit)
        );
    }
}
