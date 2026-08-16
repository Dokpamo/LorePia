//! UI-safe adapter over Lorepia's existing high-level Core contract.
//!
//! This crate deliberately does not redefine product behavior. Collection
//! methods remain whole-`Vec` operations, message mutations retain
//! `expected_head`, and chat events retain Core's current event version and
//! variants. Host paths and credentials are represented only by
//! non-serializable Rust boundary types.

/// Version of the serialized webview-facing shell contract.
///
/// Version 2 adds the typed prompt-orchestration, creator-content, and
/// redacted plan-review surfaces.
pub const SHELL_API_VERSION: u32 = 2;

mod api;
mod asset;
mod catalog;
mod discovery;
mod dto;
mod error;
mod interaction;
mod model_sync;
mod module_lifecycle;
mod orchestration;
mod package;
mod persona;
mod provider;
mod sensitive;
mod stream;

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support;

pub use api::{
    ConversationGreetingSelectionInput, CreateConversationBranchInput, CreateConversationInput,
    EditUserMessageInput, GenerationSelectionInput, RegenerateAssistantMessageInput,
    RemoveMessageInput, SelectConversationBranchInput, SendMessageInput, SetConversationModeInput,
    ShellApi, StartedGeneration, StartedMessageAction,
};
pub use asset::{
    AssetDeliveryDto, AssetDeliveryKindDto, AssetDeliverySelector, AssetProtocolRange,
    ResolveAssetDeliveryInput,
};
pub use catalog::{
    ProviderCatalogActivationSummaryDto, ProviderCatalogDiffDto, ProviderCatalogHistoryDto,
    ProviderCatalogImportPlanDto, ProviderCatalogImportResultDto, ProviderCatalogImportReviewDto,
    ProviderCatalogRevisionSummaryDto, ProviderCatalogRollbackPlanDto,
    ProviderCatalogRollbackResultDto, ProviderCatalogStatusDto,
};
pub use discovery::{
    BeginProviderDiscoveryCurlInput, BeginProviderDiscoveryInput,
    BeginProviderDiscoverySourceInput, ContinueProviderDiscoveryActionInput,
    ContinueProviderDiscoveryInput, DiscoveryActionRequiredDto, DiscoveryApprovalBindingDto,
    DiscoveryApprovalGrantDto, DiscoveryApprovalRecordDto,
    DiscoveryAssistantConflictDispositionDto, DiscoveryAssistantDraftFieldDto,
    DiscoveryAssistantDraftReviewDto, DiscoveryAssistantEndpointDto,
    DiscoveryAssistantEvidenceConflictDto, DiscoveryAssistantEvidenceMappingDto,
    DiscoveryAssistantFailureKindInput, DiscoveryAssistantFieldConfidenceDto,
    DiscoveryAssistantHostActionDto, DiscoveryAssistantInterruptionOutcomeInput,
    DiscoveryAssistantManifestDraftDto, DiscoveryAssistantManifestDto,
    DiscoveryAssistantManifestSourceDto, DiscoveryAssistantQuestionDto,
    DiscoveryAssistantResumeBoundaryDto, DiscoveryCandidateDto, DiscoveryCandidateSummaryDto,
    DiscoveryCompensationRecordDto, DiscoveryEvidenceDto, DiscoveryFailureDto,
    DiscoveryOutboxEventDto, DiscoveryProgressDto, DiscoveryRecoveryResultDto,
    DiscoveryReviewChangeDto, DiscoveryReviewDto, DiscoveryStepDto,
    DiscoveryUnknownOutcomeResolutionInput, ProviderDiscoveryApprovalProposalDto,
    ProviderDiscoveryConnectionOptionsDto, ProviderDiscoveryConnectionOptionsInput,
    ProviderDiscoveryCredentialAuthorityDto, ProviderDiscoveryCredentialCommitConfirmationDto,
    ProviderDiscoveryCredentialInstallContextDto, ProviderDiscoveryCredentialLeaseContextDto,
    ProviderDiscoveryEventDto, ProviderDiscoveryReviewProposalDto, ProviderDiscoverySessionDto,
};
pub use dto::{
    BootstrapDto, CharacterDto, CharacterGreetingCatalogDto, CharacterGreetingKindDto,
    CharacterGreetingOptionDto, ChatEventDto, ChatEventKindDto, ContentKindDto,
    ConversationBranchDto, ConversationDto, ConversationModeDto, ConversationStateDto,
    GenerationStartedDto, GenerationTargetDto, GenerationUsageDto, HealthDto,
    ImportImagePreviewDto, ImportInspectionDto, ImportWarningDto, MessageActionGenerationDto,
    MessageDisplayProjectionDto, MessageDto, MessageRoleDto, MessageStatusDto,
    MessageTransformDiagnosticDto, MessageTransformDispositionDto, MessageTransformStageDto,
};
pub use error::{ShellError, ShellErrorCode, ShellResult};
pub use interaction::{
    ClaimedInteractionEffect, DecideGenerationAttemptProposalInput, DecideInteractionProposalInput,
    ExpireGenerationAttemptProposalsInput, ExpireInteractionProposalsInput,
    GenerationAttemptProposalDecisionReceiptDto, GenerationAttemptProposalDto,
    GenerationAttemptProposalExpiryReceiptDto, GenerationAttemptProposalListItemDto,
    InteractionChoiceDto, InteractionChoiceSelectionReceiptDto, InteractionChoiceStatusDto,
    InteractionEffectDeliveryDto, InteractionEffectDto, InteractionEffectHistoryCursorDto,
    InteractionEffectHistoryItemDto, InteractionEffectHistoryPageDto,
    InteractionEffectProjectionRejectionReasonDto, InteractionProposalDecisionInput,
    InteractionProposalDecisionReceiptDto, InteractionProposalDto,
    InteractionProposalExpiryReceiptDto, InteractionProposalListItemDto,
    InteractionProposalProjectionRejectionReasonDto, InteractionProposalStatusDto,
    InteractionReopenSnapshotDto, InteractionUiRegionDto, ListGenerationAttemptProposalsInput,
    ListInteractionEffectHistoryInput, ListInteractionProposalsInput,
    ListRecentReopenInteractionEffectsInput, ListRetryableGenerationAttemptsInput,
    RetryableGenerationAttemptDto, RetryableGenerationAttemptStatusDto,
    SubmitInteractionChoiceInput,
};
pub use model_sync::{
    ModelSyncDiffDto, ModelSyncEventDto, ModelSyncFailureDto, ModelSyncJobDto,
    ModelSyncProgressDto, ModelSyncReviewDto, ModelSyncSourceProvenanceDto, ModelSyncStartedDto,
};
pub use module_lifecycle::{
    ActivateContentModuleInput, ApplyContentModuleRollbackInput, ContentModuleActivationPlanDto,
    ContentModuleActivationReceiptDto, ContentModuleActivationReviewDto,
    ContentModuleDeactivationReceiptDto, ContentModuleDeactivationReviewDto,
    ContentModuleImportApprovalCandidateDto, ContentModuleLifecycleBindingDto,
    ContentModuleLifecycleBindingItemDto, ContentModuleLifecycleBindingsDto,
    ContentModuleLifecycleCandidateDto, ContentModuleLifecycleCandidatesDto,
    ContentModuleLifecycleRevisionDto, ContentModuleRollbackPlanDto,
    ContentModuleRollbackReviewDto, ContentModuleScopeTargetDto, DeactivateContentModuleInput,
    ListContentModuleLifecycleBindingsInput, ListContentModuleLifecycleCandidatesInput,
    ResolveContentModuleActivationInput, ResolveContentModuleRollbackInput,
    ReviewContentModuleActivationInput, ReviewContentModuleDeactivationInput,
    ReviewContentModuleRollbackInput,
};
pub use orchestration::{
    ApplyPromptPresetRollbackInput, ContentModuleDto, ContentModuleRevisionDiffDto,
    ContentModuleRevisionListDto, ContentModuleRevisionSummaryDto, ContentShareGateDto,
    CreatorContentModuleDocumentDto, CreatorContentModuleMetadataDto, CreatorControlProjectionDto,
    CreatorInteractionRuleDocumentDto, CreatorInteractionRuleSetDocumentDto,
    CreatorKnowledgeBookDocumentDto, CreatorKnowledgeEntryDocumentDto,
    CreatorMemoryProfileDocumentDto, CreatorModulePromptFragmentDocumentDto,
    CreatorOrchestrationProvenanceDto, CreatorOrchestrationSourceKind, CreatorPromptBlockAuthority,
    CreatorPromptBlockDocumentDto, CreatorPromptBlockPlacementZone, CreatorPromptPresetDocumentDto,
    CreatorPromptPresetMetadataDto, CreatorTransformRuleDocumentDto,
    CreatorTransformSetDocumentDto, DeleteContentModuleInput, DeleteInteractionRuleSetInput,
    DeleteKnowledgeBookInput, DeleteMemoryProfileInput, DeleteMemoryRecordInput,
    DeletePromptPresetInput, DeleteTaskProfileInput, DeleteTransformSetInput,
    DiffContentModuleRevisionsInput, DiffPromptPresetRevisionsInput,
    EvaluateContentModuleShareInput, ExpertPromptPreviewDto, ExplainPromptPlanInput,
    GetContentModuleInput, GetInteractionRuleSetInput, GetKnowledgeBookInput,
    GetMemoryProfileInput, GetMemoryRecordInput, GetOrchestrationWorkspaceInput,
    GetPromptPresetInput, GetTaskProfileInput, GetTransformSetInput, InteractionRuleSetDto,
    KnowledgeBookDto, KnowledgeSelectionEvidenceDto, KnowledgeSimulationDto,
    KnowledgeTokenEstimateInput, ListContentModuleBindingsInput, ListContentModuleRevisionsInput,
    ListMemoryRecordsInput, ListPromptPresetBindingsInput, ListPromptPresetRevisionsInput,
    ListRetryableMemoryQueryEmbeddingsInput, MemoryJobRetryKindDto, MemoryJobRetryReceiptDto,
    MemoryJobRetryStatusDto, MemoryProfileDto, MemoryQueryEmbeddingRetryCandidateDto,
    MemoryQueryEmbeddingRetryStatusDto, MemoryRecordExclusionScopeDto, MemoryRecordListDto,
    MemoryRecordPatchDto, MemoryRecordProjectionDto, MemoryRecordSourceNavigationDto,
    ModuleBindingDto, OrchestrationWorkspaceSnapshotDto, PatchMemoryRecordInput,
    PreviewTransformRuleInput, PromptAppliedParameterPreviewDto,
    PromptAppliedParameterValueKindDto, PromptBlockProjectionDto, PromptBlockResolutionTraceDto,
    PromptBlockSourceTraceDto, PromptCacheDirectivePreviewDto, PromptDiffEntryDto,
    PromptEvidenceExclusionCodeDto, PromptKnowledgeSelectionEvidenceDto,
    PromptKnowledgeSelectionReasonDto, PromptMemorySelectionEvidenceDto,
    PromptMemorySelectionReasonDto, PromptOverflowTraceDto, PromptPlanMessagePreviewDto,
    PromptPlanPreviewDto, PromptPresetBindingDto, PromptPresetRevisionDiffDto,
    PromptPresetRevisionListDto, PromptPresetRevisionSummaryDto, PromptPresetRollbackReceiptDto,
    PromptPresetRollbackReviewDto, PromptPresetSummaryDto, PromptProviderMessagePreviewDto,
    PromptResolutionTraceDto, PromptRoleMappingTraceDto, PromptWarningCodeDto,
    ReorderPromptBlocksInput, ReorderPromptBlocksResultDto, ResolvePromptPreviewInput,
    RetryInterruptedMemoryJobInput, RetryMemoryQueryEmbeddingInput,
    ReviewPromptPresetRollbackInput, ReviewedPromptSendInput, RevisionedDto,
    RoomOrchestrationConfigDto, RoomOrchestrationFieldSupportDto,
    RoomOrchestrationSupportedFieldsDto, RoomReasoningEffortDto, SaveRoomOrchestrationConfigInput,
    SaveRoomOrchestrationConfigResultDto, SelectedKnowledgeEntryDto, SetMemoryRecordExclusionInput,
    SimulateKnowledgeInput, TaskProfileDto, TransformRulePreviewDto, TransformSetDto,
    UpsertContentModuleInput, UpsertInteractionRuleSetInput, UpsertKnowledgeBookInput,
    UpsertMemoryProfileInput, UpsertPromptPresetInput, UpsertTaskProfileInput,
    UpsertTransformSetInput,
};
pub use package::{
    ApprovableContentPackageCapabilityDto, ApproveContentPackageImportInput,
    ApproveContentPackageImportReceiptDto, CommitContentPackageImportInput,
    CommitContentPackageImportReceiptDto, ConfirmedContentPackageUpdateTargetDto,
    ContentPackageApprovalReviewDto, ContentPackageCapabilityDecisionDto,
    ContentPackageCapabilityDto, ContentPackageCapabilitySupportDto,
    ContentPackageComponentDispositionDto, ContentPackageComponentKindDto,
    ContentPackageComponentReviewDto, ContentPackageImportReviewDto, ContentPackageImportStatusDto,
    ContentPackageImportSummaryDto, ContentPackageInspectionReviewDto, ContentPackageIssueDto,
    ContentPackageIssueSeverityDto, ContentPackageManifestReviewDto,
    ContentPackageRedistributionStatusDto, ContentPackageSelectionReviewDto,
    ContentPackageTargetDispositionDto, ContentPackageTargetDocumentKindDto,
    ContentPackageTargetReviewDocumentDto, ContentPackageTargetReviewDto,
    ContentPackageWorkspaceDto, ContentSourceExportDescriptorDto, ContentSourceExportInput,
    ContentSourceExportKindDto, ContentSourceExportReceiptDto, DiscardContentPackageImportInput,
    ExportContentPackageInput, ListCompletedContentPackageExportsInput,
    ListPendingContentPackageImportsInput, PackageNormalizationEvidenceDto,
    PreparedContentSourceExport, ReopenContentPackageImportInput, SelectContentPackageImportInput,
    SelectContentPackageImportReceiptDto,
};
pub use persona::{
    ClearConversationPersonaInput, ConversationPersonaSelectionDto, CreatePersonaInput,
    DeletePersonaInput, GetConversationPersonaSelectionInput, GetPersonaInput, ListPersonasInput,
    PersonaDeletionReceiptDto, PersonaDocumentDto, PersonaDto, PersonaListPageDto,
    PersonaPageCursorDto, SelectConversationPersonaInput, SelectedPersonaSnapshotDto,
    UpdatePersonaInput,
};
pub use provider::{
    ApiFamilyInput, AppSettingsDto, AuthBindingDto, CacheTtlBoundsDto, CapabilityKeyInput,
    CapabilityObservationDto, CapabilityOverrideStatusInput, CapabilityOverrideValueInput,
    CapabilityValueDto, ConnectionConfigEntryDto, ConnectionConfigValueDto, ConnectionFieldSpecDto,
    CreateProviderConnectionInput, CredentialScopeDto, EffectiveCapabilityDto, GenerationPresetDto,
    GenerationPresetInput, GenerationReasoningSettingsDto, ModelAvailabilityInput,
    ModelRouteConfigDto, ModelRouteDto, ParameterChoiceDto, ParameterConditionDto,
    ParameterConflictDto, ParameterIssueDto, ParameterLiteralDto, ParameterSpecDto,
    ParameterValueDto, ParameterValueStateDto, PromptCacheControlDto, PromptCacheSettingsDto,
    PromptCacheTtlDto, ProviderConnectionDto, ProviderCredentialAccessAuthorityContext,
    ProviderCredentialOperationContext, ProviderCredentialOperationKindInput,
    ProviderCredentialSlotGarbageContext, ProviderCredentialSlotStatusInput,
    ProviderLocalNetworkApprovalDto, ProviderLocalNetworkApprovalInput, ProviderNetworkModeInput,
    ProviderParameterMappingDto, ProviderProfileDto, ProviderTemplateDto, ReasoningControlDto,
    RequestBodyFieldDto, RequestBodyShapeDto, RequestPreviewDto, TokenBudgetBoundsDto,
    UpdateProviderConnectionInput, UpsertCapabilityOverrideInput, UpsertModelRouteInput,
};
pub use sensitive::{
    GenerationCredential, SecretCredential, SecretProviderCurl, SignedCatalogEnvelope,
    StagedImportFile, TaskCredentialLease, TaskCredentialRead, TaskCredentialReader,
};
pub use stream::{ChatEventStream, ChatStreamItem, ReconcileReason, ReconciliationRequiredDto};
