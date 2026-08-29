//! `SQLite` and content-addressed file persistence.

mod catalog;
mod content_export;
mod cutover;
mod database;
mod discovery;
mod discovery_repository;
mod generation_attempt;
mod interaction_repository;
mod knowledge_embedding;
mod lifecycle_outbox;
mod memory_query_embedding;
mod memory_queue;
mod message_display_projection;
mod model_sync;
mod orchestration;
mod package_repository;
mod persona_repository;
mod portable_runtime_state;
mod provider_credential_repository;
mod runtime_model_audit;
mod verified_asset_cache;

pub use catalog::{
    CatalogActivationKind, CatalogActivationRecord, CatalogImportCommit, CatalogRollbackCommit,
    CatalogSnapshotSource, CatalogStateExpectation, CatalogStorageError, NewCatalogSnapshot,
    NewSignedCatalogUpdate, StoredCatalogActiveSnapshot, StoredCatalogRevisionGuard,
    StoredCatalogSnapshot, StoredCatalogState, StoredSignedCatalogUpdate,
};
pub use content_export::VerifiedContentSource;
pub use database::{
    ApprovedAssetRange, DatabaseConnectionMetrics, DatabaseStats, MessageGenerationAction,
    MessageGenerationActionContext, StagedAssetImport, Storage,
};
pub use discovery::{DurableOperationOutcome, PersistDiscoveryTransition};
pub use discovery_repository::{
    DiscoveredProviderGraph, DiscoveryActionReplay, DiscoveryCommitAttemptRecord,
    DiscoveryCommitPhase, DiscoveryCompensationRecord, DiscoveryCompensationStatus,
    DiscoveryCompletedOperationWrite, DiscoveryEvidenceKind, DiscoveryEvidenceRecord,
    DiscoveryJsonUpdate, DiscoveryNativeCredentialExecutionRecord,
    DiscoveryNativeCredentialExecutionReservation, DiscoveryNativeCredentialStoreAttemptStart,
    DiscoveryNativeNoEffectAttestationKind, DiscoveryNativeNoEffectAttestationRecord,
    DiscoveryNativeNoEffectAttestationWrite, DiscoveryNativeRecoveryOwner,
    DiscoveryOperationRecord, DiscoveryOperationStatus, DiscoveryOutboxEvent,
    DiscoveryRecoveryResult, DiscoverySessionSnapshot, DiscoveryTransitionWrite,
    PreparedDiscoveryCommit, PreparedDiscoveryCompensationStep, StoredDiscoveryCandidate,
};
pub use generation_attempt::{
    GenerationApprovalEvidence, GenerationAttemptDerivedClosure,
    GenerationAttemptDerivedGuardAudit, GenerationAttemptDerivedGuardKind,
    GenerationAttemptDerivedTransition, GenerationAttemptInput, GenerationAttemptStatus,
    GenerationBeforeEventEvidence, GenerationDispatchSeal, GenerationPromptQuickSettingsAuthority,
    GenerationPromptSelectionAuthority, GenerationProviderTargetAuthority,
    InteractionEvaluationAssetDiagnostic, InteractionEvaluationKnowledgeRevision,
    InteractionEvaluationLimits, InteractionEvaluationSeal, InteractionEvaluationTemplateValues,
    RetryableGenerationAttemptProjection, StoredGenerationAttempt, deterministic_generation_id,
    deterministic_proposed_branch_id, generation_approval_evidence_sha256,
    generation_attempt_derived_chain_sha256, generation_attempt_derived_closure_sha256,
    generation_attempt_derived_event_sha256, generation_attempt_derived_guard_evidence_sha256,
    generation_attempt_derived_transition_commit_sha256,
    generation_attempt_derived_transition_sha256, generation_attempt_sha256,
    generation_before_event_evidence_sha256, generation_dispatch_seal_sha256,
    generation_prompt_selection_authority_sha256, interaction_evaluation_seal_sha256,
};
pub use interaction_repository::{
    GenerationAttemptBeforeReviewCommit, GenerationAttemptProposalDecision,
    GenerationAttemptProposalDecisionCommit, GenerationAttemptProposalDecisionReceipt,
    InteractionActionResultStatus, InteractionActionResultWrite, InteractionChoiceEffectStatus,
    InteractionChoiceExpirationCommit, InteractionChoiceSelectionCommit,
    InteractionChoiceSelectionReceipt, InteractionDerivedEventCommit,
    InteractionDerivedEventSupervisorStatus, InteractionDerivedEventWrite,
    InteractionDerivedOccurrenceCommit, InteractionEffectHistoryCursor, InteractionEventCommit,
    InteractionEventOccurrenceLookup, InteractionKnowledgeBinding,
    InteractionPolicyRuleSetRevision, InteractionPolicySnapshot, InteractionProposalApprovalCommit,
    InteractionProposalApprovalReceipt, InteractionProposalExpiryCommit,
    InteractionProposalExpiryReceipt, InteractionProposalRejectionCommit, InteractionProposalWrite,
    InteractionStateKey, StoredGenerationAttemptBeforeReview,
    StoredGenerationAttemptInteractionAggregate, StoredGenerationAttemptInteractionBoundary,
    StoredGenerationAttemptProposal, StoredInteractionDerivedEvent,
    StoredInteractionDerivedEventQuarantine, StoredInteractionEffect,
    StoredInteractionEffectHistory, StoredInteractionEvent, StoredInteractionProposal,
    StoredInteractionState, StoredInteractionStateCheckpoint, interaction_action_sha256,
    interaction_policy_sha256, interaction_proposal_review_sha256,
    interaction_state_key_for_branch, interaction_state_snapshot_sha256,
};
pub use knowledge_embedding::{
    KnowledgeEmbeddingCoverageQuery, KnowledgeEmbeddingCoverageResult, KnowledgeEmbeddingMatch,
    KnowledgeEmbeddingQuery, KnowledgeEmbeddingQueryResult, KnowledgeEmbeddingWrite,
};
pub use lifecycle_outbox::{LifecycleOccurrenceKind, StoredLifecycleOccurrence};
pub use lorepia_orchestration::no_applied_module_runtime_plan_sha256;
pub use memory_query_embedding::{
    MemoryQueryEmbeddingEnqueueResult, MemoryQueryEmbeddingIntent, MemoryQueryEmbeddingStatus,
    StoredMemoryQueryEmbedding,
};
pub use memory_queue::{
    MemoryEmbeddingJobCompletion, MemoryEmbeddingJobInput, MemoryEmbeddingJobSeed,
    MemoryEmbeddingMatch, MemoryEmbeddingQuery, MemoryJobEnqueue, MemoryJobEnqueueResult,
    MemoryJobFinish, MemoryJobInterruption, MemoryRecordExclusionScope, MemoryRecordUserPatch,
    MemorySummaryJobCompletion, StoredMemoryEmbedding, StoredMemoryJobQueueEntry,
    memory_job_input_fingerprint,
};
pub use message_display_projection::{
    MAX_MESSAGE_DISPLAY_PROJECTION_BYTES, MAX_MESSAGE_DISPLAY_PROJECTION_CHARS,
    MAX_MESSAGE_TRANSFORM_APPLICATIONS, MAX_MESSAGE_TRANSFORM_PIPELINE_FAILURES,
    MessageDisplayProjectionWrite, MessageTransformApplicationWrite, MessageTransformDiagnostic,
    MessageTransformDisposition, MessageTransformPipelineFailureWrite, MessageTransformStage,
    StoredMessageDisplayProjection,
};
pub use model_sync::validate_provider_api_route_metadata;
pub use orchestration::{
    ActiveContentModuleRevision, ContentModuleRevisionDiff, GenerationPromptPlanRecord,
    KnowledgeActivationLog, MAX_MEMORY_EMBEDDING_DIMENSIONS, MAX_ORCHESTRATION_JSON_BYTES,
    MAX_ORCHESTRATION_JSON_CHARS, MAX_ORCHESTRATION_JSON_DEPTH, MAX_ORCHESTRATION_JSON_NODES,
    MemoryEmbeddingRecord, MemoryInvalidationResult, MemoryRecordAtHeadEvidence,
    MemoryRecordsAtHeadSelection, MemoryRecordsAtHeadSnapshot, ModuleRevisionComponentSnapshot,
    ObjectRevision, OrchestrationDatabaseStats, PackageCommitDocument, PackageCommitInput,
    PackageImportRecord, PackageImportStatus, PackageSourceRecord, PersonaCatalogPage,
    PromptPresetBinding, PromptPresetModuleDependency, PromptPresetRevisionDiff,
    PromptPresetRollbackApproval, PromptPresetRollbackCommit, PromptPresetRollbackReview,
    PromptResponseLength, ProviderRequestSnapshotRecord, RecoveredModuleActivation,
    RecoveredModuleRollback, StoredPromptMessage, StoredRevision, built_in_prompt_presets,
    memory_records_at_head_snapshot_sha256, prompt_preset_rollback_approval_sha256,
    versioned_json_sha256,
};
pub use package_repository::{
    CompletedPackageAssetAuthority, CompletedPackageAssetSourceAuthority,
    CompletedPackageAuthority, CompletedPackageComponentAuthority,
    CompletedPackageDocumentAuthority, MAX_COMPLETED_PACKAGE_EXPORTS,
    MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS, PackageCapability, PackageCapabilityDecision,
    PackageCapabilityReview, PackageCapabilitySupport, PackageDocumentCommitBinding,
    PackageDocumentTargetDisposition, PackageDocumentTargetReview, PackageImportApprovalRecord,
    PackageImportAuditEvent, PackageImportExpectation, PackageImportTargetReview,
    PackageInspectionExpectation, PackageNormalizationEvidence, PackageUpdateTargetConfirmation,
    package_capability_review_sha256, package_import_target_review_sha256,
    package_normalization_evidence_sha256, package_update_target_confirmations_sha256,
};
pub use persona_repository::ConversationPersonaSelectionState;
pub use portable_runtime_state::{
    MAX_PORTABLE_RUNTIME_STATE_BYTES, MAX_PORTABLE_RUNTIME_STATE_ROWS,
    MAX_PORTABLE_RUNTIME_STATE_TOTAL_BYTES, PortableRuntimeStatePayload,
    PortableRuntimeStateRecord, PortableRuntimeStateSaveResult, PortableRuntimeStateScope,
    PortableRuntimeStateSnapshot, PortableRuntimeStateWrite,
};
pub use provider_credential_repository::{
    ProviderCredentialAccessAuthority, ProviderCredentialObservedStatus,
    ProviderCredentialOperationKind, ProviderCredentialOperationPlan,
    ProviderCredentialOperationStatus, ProviderCredentialOutcomeCode,
    ProviderCredentialSlotGarbage, ProviderCredentialSlotGarbageStatus,
    StoredProviderCredentialOperation, provider_credential_binding_sha256_for_connection,
};
pub use runtime_model_audit::{
    RuntimeModelAuditFinish, RuntimeModelAuditStart, RuntimeModelAuditStatus,
    RuntimeModelCapability, StoredRuntimeModelAudit,
};
