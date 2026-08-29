//! High-level application API consumed by every platform binding.

mod app;
mod asset_delivery;
mod catalog;
mod config;
mod content_export;
mod content_package;
mod message_display_projection;
mod module_orchestration;
mod orchestration;
mod orchestration_runtime;
mod persona;
mod provider_credential;
mod provider_discovery;
mod provider_discovery_deterministic;

pub use app::{
    ConnectionBoundCredential, Core, EffectiveCapability, GenerationCredentialAdmissionLease,
    GenerationEventSubscription, GenerationOperationContext, MAX_GENERATION_OPERATION_NONCE_BYTES,
    MAX_GENERATION_OPERATION_NONCE_CHARS, ProviderModelRefreshProvenance,
    ProviderModelRefreshResult, ProviderTemplateView, RuntimeGenerationAuditContext,
    RuntimeGenerationCapability, RuntimePromptMessage,
};
pub use asset_delivery::{AssetDeliveryDescriptor, AssetDeliveryKind, AssetDeliveryRange};
pub use catalog::{
    PROVIDER_CATALOG_HISTORY_SCHEMA_VERSION, PROVIDER_CATALOG_IMPORT_PLAN_SCHEMA_VERSION,
    PROVIDER_CATALOG_ROLLBACK_PLAN_SCHEMA_VERSION, PROVIDER_CATALOG_STATUS_SCHEMA_VERSION,
    ProviderCatalogActivationKind, ProviderCatalogActivationSummary, ProviderCatalogHistory,
    ProviderCatalogImportPlan, ProviderCatalogImportResult, ProviderCatalogImportReview,
    ProviderCatalogRevisionSummary, ProviderCatalogRollbackPlan, ProviderCatalogRollbackResult,
    ProviderCatalogStatus,
};
pub use config::{CoreConfig, DiscoveryRecoveryOwner};
pub use content_export::{
    ContentSourceExportDescriptor, ContentSourceExportKind, ContentSourceExportSelector,
    PreparedContentSourceExport,
};
pub use content_package::{
    ContentPackageApprovalReceipt, ContentPackageApprovalRequest, ContentPackageCommitReceipt,
    ContentPackageCommitRequest, ContentPackageDiscardRequest, ContentPackageImportApprovalReview,
    ContentPackageImportInspection, ContentPackageImportReview,
    ContentPackageImportSelectionReview, ContentPackageSelectionReceipt,
    ContentPackageSelectionRequest,
};
pub use lorepia_chat::{CHAT_EVENT_VERSION, ChatEvent, ChatEventKind};
pub use lorepia_content::{
    ContentPackageComponent, ContentPackageComponentKind, ContentPackageComponentState,
    ContentPackageInspection, ContentPackageManifest, ContentPackageSelectionPlan, PackageConflict,
};
pub use lorepia_domain::discovery::*;
pub use lorepia_domain::orchestration::*;
pub use lorepia_domain::{
    ApiFamily, AppSettings, AssetDescriptor, AuthBinding, BoundedJson, CanonicalOrigin,
    CapabilityKey, CapabilityObservation, CapabilityValue, Character, CharacterContentV1,
    CharacterGreetingCatalog, CharacterGreetingKind, CharacterGreetingOption, Confidence,
    ConnectionConfig, ConnectionConfigEntry, ConnectionConfigValue, ConnectionFieldSpec,
    ConnectionFieldType, ConnectionStatus, ContentKind, Conversation, ConversationBranch,
    ConversationBranchId, ConversationGreetingBinding, ConversationId, ConversationMode,
    ConversationStart, ConversationState, CoreError, CoreErrorCode, CoreResult,
    CredentialRedirectPolicy, CredentialRef, CredentialScope, DecoderId, DiscoverySessionId,
    EndpointPath, EvidenceId, GenerationId, GenerationPreset, GenerationPresetId,
    GenerationPromptCacheMode, GenerationPromptCacheSettings, GenerationPromptCacheTtl,
    GenerationReasoningEffort, GenerationReasoningMode, GenerationReasoningSettings,
    GenerationReasoningSummary, GenerationRecord, GenerationStatus, GenerationTarget,
    GenerationUsage, HeaderName, HealthReport, HttpMethod, HttpUrl, ImportImagePreview,
    ImportInspection, ImportRegexRulePhase, ImportWarning, InspectionId, ManifestSourceKind,
    Message, MessageActionGeneration, MessageId, MessageRole, MessageStatus, ModelAvailability,
    ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId, ModelSyncDiff, ModelSyncEvent,
    ModelSyncFailure, ModelSyncJob, ModelSyncJobId, ModelSyncProgress, ModelSyncReview,
    ModelSyncSourceProvenance, ModelSyncState, ObservationId, ObservationSource, ParameterChoice,
    ParameterCondition, ParameterConditionOperator, ParameterConflict, ParameterConflictKind,
    ParameterDefaultMode, ParameterId, ParameterLiteral, ParameterSpec, ParameterType,
    ParameterValue, ParameterValueState, PortableTransformPhase, ProviderConnection,
    ProviderConnectionDraft, ProviderConnectionId, ProviderLocalNetworkApproval,
    ProviderNetworkMode, ProviderParameterMapping, ProviderParameterTarget, ProviderProfile,
    ProviderTemplate, ProviderTemplateId, Sha256Digest, SupportStatus, TemplateSource,
    ToolCallArgumentsDelta, ToolCallId, ToolName, ToolPolicy, UiParameterLevel,
};
pub use lorepia_domain::{MODEL_SYNC_EVENT_VERSION, MODEL_SYNC_REDACTION_VERSION};
pub use lorepia_orchestration::{
    ApprovedModuleActivationPlan, ApprovedModuleRollbackPlan, IgnoredModuleBindingReason,
    KnowledgeSelection, MemorySelection, MemorySelectionEvidence, MemorySemanticScore,
    ModuleActivationApproval, ModuleActivationPlan, ModuleActivationReview, ModuleCandidateSource,
    ModuleCapabilityDiff, ModuleComponentChange, ModuleComponentChangeKind,
    ModuleImportApprovalEvidence, ModuleMergeResolutionSet, ModuleResolutionContext,
    ModuleRevisionDiff, ModuleRollbackBlocker, ModuleRollbackPlan, ModuleRollbackReview,
    PackageComponentDisposition, PackageComponentKind, PackageIssueSeverity, PackageReview,
    RedistributionStatus, ResolvedModuleComponent, ReviewedModuleCandidate,
    ReviewedModuleComponent, ReviewedModuleImportApproval, SelectedKnowledgeEntry,
    SelectedMemoryRecord, TransformDiff, TransformFailure, TransformFailureCode, TransformResult,
    TransformRuleReport, TransformRuleStatus,
};
pub use lorepia_providers::catalog::{
    CatalogChangeKind, CatalogDiffDto, CatalogRevisionSnapshot, ManifestChangedSection,
    ManifestDiffDto, ModelChangedSection, ModelMetadataDiffDto,
};
pub use lorepia_providers::parameter_mapping::{
    CacheTtlBounds, ParameterIssue, ParameterIssueCode, PromptCacheControlModel, PromptCacheMode,
    PromptCacheSettings, PromptCacheTtl, ReasoningControlModel, ReasoningEffort, ReasoningMode,
    ReasoningSettings, ReasoningSummaryMode, TokenBudgetBounds, UiControlState, UiFieldState,
};
pub use lorepia_providers::setup_assistant::{
    AssistantBudget, AssistantCallEstimate, AssistantConsentRequest, AssistantDraftReview,
    AssistantFailureKind, AssistantHostAction, AssistantManifestDraft, AssistantPromptPackage,
    AssistantState, AssistantToolCall, AssistantToolResult, AssistantTurn, ConfidenceLevel,
    ConflictDisposition, DraftField, DraftPersistence, DraftReviewCheck, DraftReviewRequirements,
    EvidenceConflict, FieldConfidence, FieldEvidenceMapping, UnresolvedQuestion,
};
pub use lorepia_providers::{
    BuiltInTemplateId, CurlAuthHint, ParsedCurlEvidence, ProviderCacheBoundaryCompilation,
    ProviderCacheBoundaryDisposition, ProviderCacheBoundaryStrategy, ProviderCacheBoundaryWarning,
    ProviderPromptPlacement, ProviderWireRole, RequestBodyField, RequestBodyShape, RequestPreview,
    SecretBytes, SecretCurlInput,
};
pub use lorepia_storage::StoredInteractionEvent;
pub use lorepia_storage::{
    CompletedPackageAssetAuthority, CompletedPackageAssetSourceAuthority,
    CompletedPackageAuthority, CompletedPackageComponentAuthority,
    CompletedPackageDocumentAuthority, ContentModuleRevisionDiff,
    ConversationPersonaSelectionState, DatabaseStats, DiscoveryCommitPhase,
    DiscoveryCompensationRecord, DiscoveryCompensationStatus, DiscoveryEvidenceKind,
    DiscoveryEvidenceRecord, DiscoveryOperationRecord, DiscoveryOperationStatus,
    DiscoveryOutboxEvent, DiscoveryRecoveryResult, DiscoverySessionSnapshot,
    GenerationApprovalEvidence, GenerationAttemptInput, GenerationAttemptStatus,
    GenerationBeforeEventEvidence, GenerationDispatchSeal, GenerationPromptPlanRecord,
    InteractionChoiceEffectStatus, InteractionChoiceSelectionReceipt,
    InteractionEffectHistoryCursor, KnowledgeActivationLog, LifecycleOccurrenceKind,
    MAX_COMPLETED_PACKAGE_EXPORTS, MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS, MemoryEmbeddingRecord,
    MemoryInvalidationResult, MemoryQueryEmbeddingStatus, MemoryRecordExclusionScope,
    MemoryRecordUserPatch, MessageTransformDiagnostic, MessageTransformDisposition,
    MessageTransformStage, ObjectRevision, PackageCapability, PackageCapabilityDecision,
    PackageCapabilityReview, PackageCapabilitySupport, PackageDocumentTargetDisposition,
    PackageDocumentTargetReview, PackageImportRecord, PackageImportStatus,
    PackageImportTargetReview, PackageNormalizationEvidence, PackageSourceRecord,
    PackageUpdateTargetConfirmation, PromptPresetBinding, PromptPresetRevisionDiff,
    PromptPresetRollbackApproval, PromptPresetRollbackReview, PromptResponseLength,
    ProviderCredentialAccessAuthority, ProviderCredentialObservedStatus,
    ProviderCredentialOperationKind, ProviderCredentialOperationPlan,
    ProviderCredentialOperationStatus, ProviderCredentialOutcomeCode,
    ProviderCredentialSlotGarbage, ProviderCredentialSlotGarbageStatus,
    RetryableGenerationAttemptProjection, StoredDiscoveryCandidate, StoredGenerationAttempt,
    StoredInteractionEffect, StoredInteractionEffectHistory, StoredInteractionProposal,
    StoredPromptMessage, StoredProviderCredentialOperation, StoredRevision,
    package_update_target_confirmations_sha256, provider_credential_binding_sha256_for_connection,
};
pub use message_display_projection::MessagePresentation;
pub use module_orchestration::{
    ApprovedContentModuleComponent, ContentModuleActivationReceipt,
    ContentModuleActivationReceiptPreflight, ContentModuleActivationRequest,
    ContentModuleActivationReviewPresentation, ContentModuleActivationRevisionReview,
    ContentModuleBindingDraft, ContentModuleDeactivationReceipt, ContentModuleDeactivationRequest,
    ContentModuleDeactivationReview, ContentModuleImportApprovalCandidate,
    ContentModuleRevisionSummary, ContentModuleRollbackApplyRequest, ContentModuleRollbackPlan,
    ContentModuleRollbackResolutionRequest, ContentModuleRollbackReview,
    ContentModuleRollbackReviewPresentation, ContentModuleRuntimeBindingDisposition,
    ContentModuleRuntimeBindingSummary, ContentModuleRuntimeTarget, ContentModuleRuntimeWorkspace,
    MAX_CONTENT_MODULE_IMPORT_APPROVAL_CANDIDATES, MAX_CONTENT_MODULE_REVISION_SUMMARIES,
};
pub use orchestration::{
    ContentShareGate, CreatorControlValue, ExpertPromptPreview, KnowledgeSimulationRequest,
    KnowledgeTokenEstimate, MemoryRetrievalRequest, PromptAppliedParameterPreview, PromptDiffEntry,
    PromptEffectiveMessageContentPreview, PromptPlanMessagePreview, PromptPlanPreview,
    PromptPlanRequest, PromptPresetRollbackApplyRequest, PromptPresetRollbackReceipt,
    PromptProviderMessagePreview, RoomOrchestrationConfig, RoomOrchestrationConfigPatch,
    TaskGenerationTargetPlan, TransformPreviewRequest,
};
pub use orchestration_runtime::{
    ClaimedMemoryJob, CoreLifecycleDeliveryReceipt, CoreLifecycleDeliveryStatus,
    CoreLifecycleDrainReceipt, EnqueueMemorySummaryRequest,
    GenerationAttemptProposalDecisionReceipt, GenerationAttemptProposalDecisionRequest,
    GenerationAttemptProposalExpiryReceipt, GenerationAttemptProposalView, InteractionEventReview,
    InteractionProposalDecisionReceipt, InteractionProposalDecisionRequest,
    InteractionReviewRequest, InteractionRuleSetRevision, InterruptedMemoryJob,
    MemoryJobEnqueueReceipt, MemoryJobExecutionResult, MemoryQueryEmbeddingRetryCandidate,
    MemoryRuntimeProvenance, RuntimeTaskTargetRevision, RuntimeTransformRevision,
    TaskCredentialBroker,
};
pub use persona::{
    ConversationPersonaClearRequest, ConversationPersonaSelectionRequest, MAX_PERSONA_LIST_LIMIT,
    PersonaCreateRequest, PersonaDeleteRequest, PersonaListCursor, PersonaListPage,
    PersonaUpdateRequest,
};
#[cfg(feature = "test-support")]
pub use provider_discovery::test_support as provider_discovery_test_support;
pub use provider_discovery::{
    ProviderCurlInspection, ProviderDiscoveryAdditionalEvidence, ProviderDiscoveryApprovalProposal,
    ProviderDiscoveryAssistantResumeAction, ProviderDiscoveryAssistantResumeBoundary,
    ProviderDiscoveryCredentialAuthority, ProviderDiscoveryCredentialCommitConfirmation,
    ProviderDiscoveryCredentialInstallContext, ProviderDiscoveryCredentialLeaseContext,
    ProviderDiscoveryCurlInput, ProviderDiscoveryReviewProposal, ProviderDiscoverySource,
    provider_discovery_action_envelope,
};

pub const CORE_API_VERSION: u32 = 9;

pub fn core_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
