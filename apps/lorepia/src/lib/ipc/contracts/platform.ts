import type {
    AssetDeliveryDto,
    CharacterDto,
    CharacterGreetingCatalogDto,
    CharacterGreetingSelectionInput,
    ResolveAssetDeliveryInput,
} from './character';

import type {
    ConversationBranchDto,
    ConversationDto,
    ConversationMode,
    ConversationStateDto,
    MessageDto,
} from './conversation';

import type {
    ChatStreamItemDto,
    DecideGenerationAttemptProposalInput,
    DecideInteractionProposalInput,
    EditUserMessageInput,
    ExpireGenerationAttemptProposalsInput,
    GenerateRuntimeTextInput,
    GenerationAttemptProposalDecisionReceiptDto,
    GenerationAttemptProposalExpiryReceiptDto,
    GenerationAttemptProposalListItemDto,
    GenerationStartedDto,
    GenerationTargetDto,
    InteractionEffectEventDto,
    InteractionProposalDecisionReceiptDto,
    ListGenerationAttemptProposalsInput,
    ListRetryableGenerationAttemptsInput,
    MessageActionGenerationDto,
    RegenerateAssistantMessageInput,
    RemoveMessageInput,
    RetryableGenerationAttemptDto,
    ReviewedPromptSendInput,
    RuntimeTextGenerationDto,
    SendMessageInput,
} from './generation';

import type { ImportInspectionDto, ImportTicketDto } from './import';

import type {
    InterruptedMemoryJobDto,
    ListInterruptedMemoryJobsInput,
    ListRetryableMemoryQueryEmbeddingsInput,
    MemoryJobRetryReceiptDto,
    MemoryQueryEmbeddingRetryCandidateDto,
    MemorySupervisorStatusDto,
    RetryInterruptedMemoryJobInput,
    RetryMemoryQueryEmbeddingInput,
} from './memory';

import type {
    CharacterRenderProfileDto,
    GetPortableRuntimeStateDto,
    PortableRuntimeStateScopeInput,
    PutPortableRuntimeStateInput,
    PutPortableRuntimeStateResultDto,
} from './portable-runtime';

import type {
    AppSettingsDto,
    CapabilityKeyInput,
    CapabilityObservationDto,
    CreateProviderConnectionInput,
    CredentialStatus,
    CredentialStatusDto,
    CredentialTargetDto,
    EffectiveCapabilityDto,
    GenerationPresetDto,
    GenerationPresetInput,
    ModelRouteDto,
    ModelSyncEventDto,
    ModelSyncJobDto,
    ModelSyncStartedDto,
    NativeCaptureStatusDto,
    ParameterSpecDto,
    PromptCacheControlDto,
    ProviderConnectionDto,
    ProviderOverviewDto,
    ProviderProfileDto,
    ProviderTemplateDto,
    ReasoningControlDto,
    RequestPreviewDto,
    UpdateProviderConnectionInput,
    UpsertCapabilityOverrideInput,
    UpsertModelRouteInput,
} from './provider';

import type {
    BeginProviderDiscoveryCurlInput,
    BeginProviderDiscoveryInput,
    CapturedProviderDiscoveryDto,
    ContinueProviderDiscoveryInput,
    DiscoveryApprovalRecordDto,
    DiscoveryAssistantFailureKindInput,
    DiscoveryAssistantHostActionDto,
    DiscoveryAssistantInterruptionOutcomeInput,
    DiscoveryAssistantResumeBoundaryDto,
    DiscoveryCandidateDto,
    DiscoveryCompensationRecordDto,
    DiscoveryEvidenceDto,
    DiscoveryOutboxEventDto,
    DiscoveryRecoveryResultDto,
    DiscoveryReviewDto,
    ProviderCatalogDiffDto,
    ProviderCatalogHistoryDto,
    ProviderCatalogImportResultDto,
    ProviderCatalogImportTicketDto,
    ProviderCatalogRollbackPlanDto,
    ProviderCatalogRollbackResultDto,
    ProviderCatalogStatusDto,
    ProviderDiscoveryApprovalProposalDto,
    ProviderDiscoveryEventDto,
    ProviderDiscoveryReviewProposalDto,
    ProviderDiscoverySessionDto,
} from './discovery';

export type PlatformKind = 'android' | 'ios' | 'macos' | 'windows';

export interface HealthDto {
    core_version: string;
    database_open: boolean;
    schema_version: number;
    data_root_writable: boolean;
    staging_writable: boolean;
    recovery_pending: boolean;
    active_jobs: number;
}

export interface PlatformCapabilitiesDto {
    file_picker: boolean;
    credential_store: boolean;
    native_menu: boolean;
    notifications: boolean;
    creator_runtime: boolean;
}

export interface BootstrapDto {
    app_version?: string;
    shell_api_version: number;
    core_version?: string;
    core_api_version: number;
    chat_event_version: number;
    creator_schema_version?: number;
    platform?: PlatformKind;
    health: HealthDto;
    capabilities?: PlatformCapabilitiesDto;
}

export interface ProviderWorkspaceDto {
    templates: ProviderTemplateDto[];
    connections: ProviderConnectionDto[];
    legacy_profiles: ProviderProfileDto[];
    routes: ModelRouteDto[];
    presets: GenerationPresetDto[];
    settings: AppSettingsDto;
    credential_statuses: Record<string, CredentialStatus>;
    request_preview: RequestPreviewDto | null;
    selected_capability_model_route_id: string | null;
    capability_observations: CapabilityObservationDto[];
    capability_parameter_specs: ParameterSpecDto[];
    effective_capability: EffectiveCapabilityDto | null;
    model_sync_jobs: ModelSyncJobDto[];
    selected_model_sync_job_id: string | null;
    model_sync_event: ModelSyncEventDto | null;
    discoveries: ProviderDiscoverySessionDto[];
    selected_discovery_id: string | null;
    discovery_candidates: DiscoveryCandidateDto[];
    discovery_evidence: DiscoveryEvidenceDto[];
    discovery_approvals: DiscoveryApprovalRecordDto[];
    discovery_review: DiscoveryReviewDto | null;
    discovery_approval_proposal: ProviderDiscoveryApprovalProposalDto | null;
    discovery_review_proposal: ProviderDiscoveryReviewProposalDto | null;
    discovery_assistant_resume_boundary: DiscoveryAssistantResumeBoundaryDto | null;
    discovery_assistant_host_action: DiscoveryAssistantHostActionDto | null;
    discovery_event: ProviderDiscoveryEventDto | null;
    discovery_compensation_steps: DiscoveryCompensationRecordDto[];
    discovery_recovery_results: DiscoveryRecoveryResultDto[];
    catalog_status: ProviderCatalogStatusDto | null;
    catalog_history: ProviderCatalogHistoryDto | null;
    pending_catalog_import: ProviderCatalogImportTicketDto | null;
    pending_catalog_rollback: ProviderCatalogRollbackPlanDto | null;
    catalog_diff: ProviderCatalogDiffDto | null;
}

export interface LorepiaClient {
    bootstrapSnapshot(): Promise<BootstrapDto>;
    getMemorySupervisorStatus(): Promise<MemorySupervisorStatusDto>;
    subscribeMemorySupervisorStatus(
        onStatus: (status: MemorySupervisorStatusDto) => void,
    ): Promise<() => void>;

    listCharacters(): Promise<CharacterDto[]>;
    getCharacter(characterId: string): Promise<CharacterDto>;
    getCharacterGreetingCatalog(characterId: string): Promise<CharacterGreetingCatalogDto>;
    getCharacterRenderProfile?(characterId: string): Promise<CharacterRenderProfileDto>;
    resolveAssetDelivery(input: ResolveAssetDeliveryInput): Promise<AssetDeliveryDto>;
    listInteractionEffects(): Promise<InteractionEffectEventDto[]>;
    acknowledgeInteractionEffect(deliveryId: string): Promise<void>;
    retryInteractionEffect(deliveryId: string): Promise<void>;
    decideInteractionProposal(
        input: DecideInteractionProposalInput,
    ): Promise<InteractionProposalDecisionReceiptDto>;
    expireGenerationAttemptProposals(
        input: ExpireGenerationAttemptProposalsInput,
    ): Promise<GenerationAttemptProposalExpiryReceiptDto>;
    listGenerationAttemptProposals(
        input: ListGenerationAttemptProposalsInput,
    ): Promise<GenerationAttemptProposalListItemDto[]>;
    listRetryableGenerationAttempts(
        input: ListRetryableGenerationAttemptsInput,
    ): Promise<RetryableGenerationAttemptDto[]>;
    decideGenerationAttemptProposal(
        input: DecideGenerationAttemptProposalInput,
    ): Promise<GenerationAttemptProposalDecisionReceiptDto>;
    subscribeInteractionEffects(
        onEffect: (effect: InteractionEffectEventDto) => void,
    ): Promise<() => void>;
    selectImportSource(): Promise<ImportTicketDto | null>;
    inspectImport(ticketId: string): Promise<ImportInspectionDto>;
    commitImport(inspectionId: string): Promise<CharacterDto>;
    discardImport(inspectionId: string): Promise<void>;

    listConversations(characterId: string | null): Promise<ConversationDto[]>;
    createConversation(
        characterId: string,
        title: string,
        mode: ConversationMode,
        greeting?: CharacterGreetingSelectionInput,
    ): Promise<ConversationDto>;
    openConversation(characterId: string): Promise<ConversationDto>;
    openExistingConversation(conversationId: string): Promise<ConversationDto>;
    getConversation(conversationId: string): Promise<ConversationDto>;
    getConversationState(conversationId: string): Promise<ConversationStateDto>;
    listBranches(conversationId: string): Promise<ConversationBranchDto[]>;
    createBranch(
        conversationId: string,
        fromMessageId: string | null,
        title: string | null,
    ): Promise<ConversationBranchDto>;
    selectBranch(conversationId: string, branchId: string): Promise<ConversationStateDto>;
    setConversationMode(
        conversationId: string,
        mode: ConversationMode,
    ): Promise<ConversationStateDto>;
    listBranchMessages(branchId: string): Promise<MessageDto[]>;
    listMessages(conversationId: string): Promise<MessageDto[]>;
    generateRuntimeText?(input: GenerateRuntimeTextInput): Promise<RuntimeTextGenerationDto>;
    cancelRuntimeText?(requestId: string): Promise<boolean>;
    getPortableRuntimeState?(
        scope: PortableRuntimeStateScopeInput,
    ): Promise<GetPortableRuntimeStateDto>;
    putPortableRuntimeState?(
        input: PutPortableRuntimeStateInput,
    ): Promise<PutPortableRuntimeStateResultDto>;
    listInterruptedMemoryJobs(
        input: ListInterruptedMemoryJobsInput,
    ): Promise<InterruptedMemoryJobDto[]>;
    retryInterruptedMemoryJob(
        input: RetryInterruptedMemoryJobInput,
    ): Promise<MemoryJobRetryReceiptDto>;
    listRetryableMemoryQueryEmbeddings(
        input: ListRetryableMemoryQueryEmbeddingsInput,
    ): Promise<MemoryQueryEmbeddingRetryCandidateDto[]>;
    retryMemoryQueryEmbedding(
        input: RetryMemoryQueryEmbeddingInput,
    ): Promise<MemoryQueryEmbeddingRetryCandidateDto>;

    sendMessage(
        input: SendMessageInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<GenerationStartedDto>;
    sendReviewedPrompt(
        input: ReviewedPromptSendInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<GenerationStartedDto>;
    editUserMessage(
        input: EditUserMessageInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<MessageActionGenerationDto>;
    regenerateAssistantMessage(
        input: RegenerateAssistantMessageInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<MessageActionGenerationDto>;
    removeMessageFromBranch(input: RemoveMessageInput): Promise<ConversationBranchDto>;
    cancelGeneration(generationId: string): Promise<void>;
    subscribeGeneration(
        generationId: string,
        conversationId: string,
        branchId: string,
        sequenceBaseline: number,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<void>;
    disposeChatStream(streamId: string): Promise<boolean>;

    getProviderOverview(): Promise<ProviderOverviewDto>;
    getSettings(): Promise<AppSettingsDto>;
    updateSettings(settings: AppSettingsDto): Promise<AppSettingsDto>;
    selectGenerationTarget(target: GenerationTargetDto | null): Promise<AppSettingsDto>;
    listProviderTemplates(): Promise<ProviderTemplateDto[]>;
    listProviderConnections(): Promise<ProviderConnectionDto[]>;
    createProviderConnection(input: CreateProviderConnectionInput): Promise<ProviderConnectionDto>;
    upsertProviderConnection(input: UpdateProviderConnectionInput): Promise<ProviderConnectionDto>;
    deleteProviderConnection(connectionId: string): Promise<void>;
    listProviderProfiles(): Promise<ProviderProfileDto[]>;
    listModelRoutes(connectionId: string): Promise<ModelRouteDto[]>;
    upsertModelRoute(input: UpsertModelRouteInput): Promise<ModelRouteDto>;
    deleteModelRoute(routeId: string): Promise<void>;
    listCapabilityObservations(modelRouteId: string): Promise<CapabilityObservationDto[]>;
    effectiveCapability(
        modelRouteId: string,
        key: CapabilityKeyInput,
    ): Promise<EffectiveCapabilityDto | null>;
    effectiveParameterSpecs(modelRouteId: string): Promise<ParameterSpecDto[]>;
    upsertUserCapabilityOverride(
        input: UpsertCapabilityOverrideInput,
    ): Promise<CapabilityObservationDto>;
    deleteUserCapabilityOverride(modelRouteId: string, observationId: string): Promise<void>;
    listGenerationPresets(routeId: string): Promise<GenerationPresetDto[]>;
    upsertGenerationPreset(input: GenerationPresetInput): Promise<GenerationPresetDto>;
    deleteGenerationPreset(presetId: string): Promise<void>;
    validateGenerationPresetCandidate(input: GenerationPresetInput): Promise<void>;
    renderReasoningControlForPreset(input: GenerationPresetInput): Promise<ReasoningControlDto>;
    renderPromptCacheControlForPreset(input: GenerationPresetInput): Promise<PromptCacheControlDto>;
    previewProviderRequestCandidate(input: GenerationPresetInput): Promise<RequestPreviewDto>;
    credentialStatus(target: CredentialTargetDto): Promise<CredentialStatusDto>;
    captureCredential(target: CredentialTargetDto): Promise<NativeCaptureStatusDto>;
    deleteCredential(target: CredentialTargetDto): Promise<void>;
    previewProviderRequest(target: GenerationTargetDto): Promise<RequestPreviewDto>;

    startProviderModelSync(connectionId: string): Promise<ModelSyncStartedDto>;
    getProviderModelSync(jobId: string): Promise<ModelSyncJobDto>;
    listProviderModelSyncs(connectionId: string, limit: number): Promise<ModelSyncJobDto[]>;
    approveProviderModelSync(jobId: string, reviewSha256: string): Promise<ModelSyncJobDto>;
    cancelProviderModelSync(jobId: string): Promise<ModelSyncJobDto>;
    pollProviderModelSyncEvents(jobId: string, limit: number): Promise<ModelSyncEventDto[]>;
    ackProviderModelSyncEvent(jobId: string, sequence: number): Promise<boolean>;

    beginProviderDiscovery(
        input: BeginProviderDiscoveryInput,
    ): Promise<ProviderDiscoverySessionDto>;
    beginProviderDiscoveryCurl(
        input: BeginProviderDiscoveryCurlInput,
    ): Promise<CapturedProviderDiscoveryDto>;
    listProviderDiscoveries(limit: number): Promise<ProviderDiscoverySessionDto[]>;
    getProviderDiscovery(sessionId: string): Promise<ProviderDiscoverySessionDto>;
    listProviderDiscoveryCandidates(sessionId: string): Promise<DiscoveryCandidateDto[]>;
    listProviderDiscoveryEvidence(sessionId: string): Promise<DiscoveryEvidenceDto[]>;
    listProviderDiscoveryApprovals(sessionId: string): Promise<DiscoveryApprovalRecordDto[]>;
    getProviderDiscoveryReview(sessionId: string): Promise<DiscoveryReviewDto | null>;
    getProviderDiscoveryApprovalProposal(
        sessionId: string,
    ): Promise<ProviderDiscoveryApprovalProposalDto | null>;
    getProviderDiscoveryReviewProposal(
        sessionId: string,
    ): Promise<ProviderDiscoveryReviewProposalDto | null>;
    getProviderDiscoveryAssistantResumeBoundary(
        sessionId: string,
    ): Promise<DiscoveryAssistantResumeBoundaryDto | null>;
    runProviderDiscoveryAssistantTurn(sessionId: string): Promise<DiscoveryAssistantHostActionDto>;
    resumeProviderDiscoveryAssistantCoreHostAction(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto>;
    approveProviderDiscoveryAssistantRetry(sessionId: string): Promise<ProviderDiscoverySessionDto>;
    requestProviderDiscoveryAssistantRevision(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto>;
    acceptProviderDiscoveryAssistantDraft(sessionId: string): Promise<ProviderDiscoverySessionDto>;
    recordProviderDiscoveryAssistantFailure(
        sessionId: string,
        kind: DiscoveryAssistantFailureKindInput,
        retryable: boolean,
    ): Promise<ProviderDiscoverySessionDto>;
    interruptProviderDiscoveryAssistant(
        sessionId: string,
        outcome: DiscoveryAssistantInterruptionOutcomeInput,
    ): Promise<ProviderDiscoverySessionDto>;
    restartProviderDiscoveryAssistantAfterInterruption(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto>;
    continueProviderDiscovery(
        input: ContinueProviderDiscoveryInput,
    ): Promise<ProviderDiscoverySessionDto>;
    supplyProviderDiscoveryDocumentEvidence(
        sessionId: string,
        expectedRevision: number,
        documentUrl: string,
    ): Promise<ProviderDiscoverySessionDto>;
    supplyProviderDiscoveryCurlEvidence(
        sessionId: string,
        expectedRevision: number,
    ): Promise<CapturedProviderDiscoveryDto>;
    cancelProviderDiscovery(
        sessionId: string,
        expectedRevision: number,
    ): Promise<ProviderDiscoverySessionDto>;
    commitProviderDiscovery(sessionId: string): Promise<ProviderConnectionDto>;
    pollProviderDiscoveryEvents(limit: number): Promise<DiscoveryOutboxEventDto[]>;
    pollProviderDiscoveryEventsForSession(
        sessionId: string,
        limit: number,
    ): Promise<DiscoveryOutboxEventDto[]>;
    ackProviderDiscoveryEvent(eventId: string): Promise<boolean>;
    recoverProviderDiscovery(): Promise<DiscoveryRecoveryResultDto[]>;
    listProviderDiscoveryCompensationSteps(
        commitAttemptId: string,
    ): Promise<DiscoveryCompensationRecordDto[]>;
    continueProviderDiscoveryCompensation(sessionId: string): Promise<ProviderDiscoverySessionDto>;
    resumeProviderDiscoveryCompensation(sessionId: string): Promise<ProviderDiscoverySessionDto>;

    pickProviderCatalogImport(): Promise<ProviderCatalogImportTicketDto | null>;
    activateProviderCatalogImport(ticketId: string): Promise<ProviderCatalogImportResultDto>;
    discardProviderCatalogImport(ticketId: string): Promise<void>;
    providerCatalogStatus(): Promise<ProviderCatalogStatusDto>;
    providerCatalogHistory(
        limit: number,
        beforeRevision: number | null,
        beforeStateVersion: number | null,
    ): Promise<ProviderCatalogHistoryDto>;
    diffProviderCatalogRevisions(
        fromRevision: number,
        toRevision: number,
    ): Promise<ProviderCatalogDiffDto>;
    prepareProviderCatalogRollback(targetRevision: number): Promise<ProviderCatalogRollbackPlanDto>;
    activateProviderCatalogRollback(
        plan: ProviderCatalogRollbackPlanDto,
    ): Promise<ProviderCatalogRollbackResultDto>;
}
