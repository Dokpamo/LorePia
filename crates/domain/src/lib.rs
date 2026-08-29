//! Platform-independent types shared by `LorePia`'s Rust crates.

mod capability;
mod character;
mod content;
mod conversation;
pub mod discovery;
mod error;
mod health;
mod message;
mod model_sync;
pub mod orchestration;
mod provider;
mod settings;

pub use capability::{
    CapabilityKey, CapabilityObservation, CapabilityValue, Confidence, ObservationSource,
    SupportStatus,
};
pub use character::{
    CHARACTER_CONTENT_SCHEMA_VERSION, Character, CharacterContentV1, CharacterKnowledgeBookRef,
    CharacterRuntimeProfile, ExtensionQuarantine, ExtensionQuarantineKind,
    MAX_UNKNOWN_EXTENSION_ENTRIES, MAX_UNKNOWN_EXTENSION_TOTAL_BYTES, PortableKnowledgeBook,
    PortableKnowledgeEntry, PortableKnowledgePlacement, PortableRuntimeScript,
    PortableTextTransform, PortableTransformPhase, UnknownExtensionEntry, UnknownExtensionIndex,
};
pub use content::{
    AssetDescriptor, AssetRole, AssetSource, AssetSourceKind, ContentKind,
    ImportDynamicContentReview, ImportImagePreview, ImportInspection, ImportLimits,
    ImportRegexRulePhase, ImportRegexRuleReview, ImportWarning, InspectionId, Sha256Digest,
};
pub use conversation::{
    CharacterGreetingCatalog, CharacterGreetingKind, CharacterGreetingOption, Conversation,
    ConversationBranch, ConversationBranchId, ConversationGreetingBinding, ConversationId,
    ConversationMode, ConversationStart, ConversationState, MessageActionGeneration,
};
pub use discovery::{DiscoveryCompensationTarget, DiscoveryPreviousSelection};
pub use error::{CoreError, CoreErrorCode, CoreResult};
pub use health::HealthReport;
pub use message::{
    CHARACTER_GREETING_GENERATION_ID_PREFIX, GenerationId, GenerationRecord, GenerationStatus,
    Message, MessageId, MessageRole, MessageStatus,
};
pub use model_sync::{
    MODEL_SYNC_EVENT_VERSION, MODEL_SYNC_REDACTION_VERSION, ModelSyncDiff, ModelSyncEvent,
    ModelSyncFailure, ModelSyncJob, ModelSyncProgress, ModelSyncReview, ModelSyncSourceProvenance,
    ModelSyncState,
};
pub use orchestration::*;
pub use provider::{
    AnthropicBlockText, AnthropicContentBlock, AnthropicContentBlockTopology, AnthropicToolInput,
    ApiFamily, AuthBinding, BoundedJson, CanonicalOrigin, ConnectionConfig, ConnectionConfigEntry,
    ConnectionConfigValue, ConnectionFieldSpec, ConnectionFieldType, ConnectionStatus,
    CredentialRedirectPolicy, CredentialRef, CredentialScope, DecoderId, DiscoverySessionId,
    EndpointPath, EndpointSpec, EvidenceId, GenerationPreset, GenerationPresetId,
    GenerationPromptCacheMode, GenerationPromptCacheSettings, GenerationPromptCacheTtl,
    GenerationProviderProvenance, GenerationReasoningEffort, GenerationReasoningMode,
    GenerationReasoningSettings, GenerationReasoningSummary, GenerationRequest, GenerationTarget,
    GenerationUsage, HeaderName, HttpMethod, HttpUrl, MAX_ANTHROPIC_BLOCK_TEXT_CHARS,
    MAX_ANTHROPIC_CONTENT_BLOCKS, MAX_ANTHROPIC_TOOL_INPUT_DEPTH, MAX_ANTHROPIC_TOOL_INPUT_NODES,
    MAX_BOUNDED_JSON_BYTES, MAX_BOUNDED_JSON_CHARS, MAX_OPAQUE_REASONING_ITEM_BYTES,
    MAX_OPAQUE_REASONING_SERIALIZED_BYTES, MAX_OPAQUE_REASONING_STATE_COUNT,
    MAX_OPAQUE_REASONING_TOTAL_BYTES, MAX_OPENAI_REASONING_PARTS, MAX_TOOL_ARGUMENT_DELTA_BYTES,
    MAX_TOOL_ARGUMENT_DELTA_CHARS, MAX_TOOL_CALL_ID_BYTES, MAX_TOOL_CALL_ID_CHARS,
    MAX_TOOL_NAME_BYTES, MAX_TOOL_NAME_CHARS, ManifestDecoders, ManifestEndpoints, ManifestSource,
    ManifestSourceKind, ModelAvailability, ModelMetadataSource, ModelRoute, ModelRouteConfig,
    ModelRouteId, ModelSyncJobId, ObservationId, OpaqueReasoningContext, OpaqueReasoningData,
    OpaqueReasoningItemId, OpaqueReasoningState, OpenAiResponsesReasoningItem,
    OpenRouterReasoningDetail, OpenRouterReasoningTopology, ParameterChoice, ParameterCondition,
    ParameterConditionOperator, ParameterConflict, ParameterConflictKind, ParameterDefaultMode,
    ParameterId, ParameterLiteral, ParameterSpec, ParameterType, ParameterValue,
    ParameterValueState, ProviderCapabilities, ProviderConnection, ProviderConnectionDraft,
    ProviderConnectionId, ProviderLocalNetworkApproval, ProviderManifest, ProviderNetworkMode,
    ProviderParameterMapping, ProviderParameterTarget, ProviderProfile, ProviderTemplate,
    ProviderTemplateId, TemplateSource, ToolCallArgumentsDelta, ToolCallId, ToolName, ToolPolicy,
    UiParameterLevel, validate_opaque_reasoning_states,
};
pub use settings::AppSettings;
