import { Channel, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

import type {
    ClearConversationPersonaInput,
    ConversationPersonaSelectionDto,
    CreatePersonaInput,
    DeletePersonaInput,
    GetConversationPersonaSelectionInput,
    GetPersonaInput,
    ListPersonaPageInput,
    ListPersonasInput,
    PersonaClientApi,
    PersonaDeletionReceiptDto,
    PersonaDto,
    PersonaListPageDto,
    SelectConversationPersonaInput,
    UpdatePersonaInput,
} from '../../features/personas/persona-contracts';
import type {
    ActivateContentModuleInput,
    ApplyContentModuleRollbackInput,
    ContentModuleDeactivationReceiptDto,
    ContentModuleDeactivationReviewDto,
    ContentModuleActivationPlanDto,
    ContentModuleActivationReceiptDto,
    ContentModuleActivationReviewPresentationDto,
    ContentModuleLifecycleBindingListDto,
    ContentModuleLifecycleCandidateListDto,
    ContentModuleLifecycleClientApi,
    ContentModuleRollbackPlanDto,
    ContentModuleRollbackReviewPresentationDto,
    DeactivateContentModuleInput,
    ListContentModuleLifecycleBindingsInput,
    ListContentModuleLifecycleCandidatesInput,
    ResolveContentModuleActivationInput,
    ResolveContentModuleRollbackInput,
    ReviewContentModuleActivationInput,
    ReviewContentModuleDeactivationInput,
    ReviewContentModuleRollbackInput,
} from '../../features/orchestration/module-lifecycle-contracts';
import type {
    ApplyPromptPresetRollbackInput,
    AssetDeliveryDto,
    ResolveAssetDeliveryInput,
    ApproveContentPackageImportInput,
    ApproveContentPackageImportReceiptDto,
    CommitContentPackageImportInput,
    CommitContentPackageImportReceiptDto,
    ContentPackageClientApi,
    ContentPackageImportReviewDto,
    ContentPackageImportSummaryDto,
    ContentPackageInspectionReviewDto,
    ContentPackageWorkspaceDto,
    ContentSourceExportDescriptorDto,
    ContentSourceExportInput,
    ContentSourceExportReceiptDto,
    ContentModuleRevisionDiffDocumentDto,
    ContentModuleRevisionListResultDto,
    ContentShareGateDto,
    CreatorContentModuleDocumentDto,
    CreatorInteractionRuleSetDocumentDto,
    CreatorKnowledgeBookDocumentDto,
    CreatorMemoryProfileDocumentDto,
    CreatorPromptPresetDocumentDto,
    CreatorTransformSetDocumentDto,
    DecideGenerationAttemptProposalInput,
    DecideInteractionProposalInput,
    ExpireGenerationAttemptProposalsInput,
    ExpireInteractionProposalsInput,
    DeleteContentModuleInput,
    DeleteInteractionRuleSetInput,
    DeleteKnowledgeBookInput,
    DeleteMemoryProfileInput,
    DeleteMemoryRecordRequest,
    DeletePromptPresetInput,
    DeleteTaskProfileInput,
    DeleteTransformSetInput,
    DiffContentModuleRevisionsInput,
    DiffPromptPresetRevisionsInput,
    EvaluateContentModuleShareInput,
    GetMemoryRecordInput,
    GetContentModuleInput,
    GetInteractionRuleSetInput,
    GetKnowledgeBookInput,
    GetMemoryProfileInput,
    GetPromptPresetInput,
    GetTransformSetInput,
    KnowledgeActivationResultDto,
    KnowledgeSimulationDto,
    ListContentModuleBindingsInput,
    ListContentModuleRevisionsInput,
    ListCompletedContentPackageExportsInput,
    ListMemoryRecordsInput,
    ListRetryableGenerationAttemptsInput,
    InterruptedMemoryJobDto,
    ListInterruptedMemoryJobsInput,
    MemoryJobRetryReceiptDto,
    RetryInterruptedMemoryJobInput,
    ListRetryableMemoryQueryEmbeddingsInput,
    ListPromptPresetBindingsInput,
    ListPromptPresetRevisionsInput,
    MemoryRecordDto,
    MemoryRecordListResultDto,
    MemoryQueryEmbeddingRetryCandidateDto,
    MemorySupervisorStatusDto,
    ModuleBindingDocumentDto,
    OrchestrationDocumentClientApi,
    OrchestrationClientApi,
    OrchestrationWorkspaceSnapshotDto,
    ExplainPromptPlanInput,
    PatchMemoryRecordRequest,
    PromptPlanPreviewDto,
    PromptPlanRequestInput,
    PromptPresetHistoryClientApi,
    PromptPresetRevisionDiffDto,
    PromptPresetRevisionListDto,
    PromptPresetRollbackReceiptDto,
    PromptPresetRollbackReviewDto,
    PromptPresetSummaryDto,
    PromptResolutionTraceDto,
    PreviewTransformRequest,
    PreviewTransformRuleInput,
    ReorderPromptBlocksInput,
    ReorderPromptBlocksResult,
    RetryMemoryQueryEmbeddingInput,
    ReviewPromptPresetRollbackInput,
    PromptPresetBindingDocumentDto,
    RevisionedDto,
    SetMemoryRecordExclusionRequest,
    SaveRoomOrchestrationConfigInput,
    SaveRoomOrchestrationConfigResult,
    SimulateKnowledgeActivationInput,
    SimulateKnowledgeRequest,
    TaskProfileDocumentDto,
    TransformPreviewDto,
    TransformRulePreviewDto,
    UpsertContentModuleInput,
    UpsertInteractionRuleSetInput,
    UpsertKnowledgeBookInput,
    UpsertMemoryProfileInput,
    UpsertTaskProfileInput,
    UpsertPromptPresetInput,
    UpsertTransformSetInput,
    BootstrapDto,
    BeginProviderDiscoveryCurlInput,
    BeginProviderDiscoveryInput,
    CapturedProviderDiscoveryDto,
    CapabilityKeyInput,
    CapabilityObservationDto,
    CharacterDto,
    CharacterGreetingCatalogDto,
    CharacterGreetingSelectionInput,
    CharacterRenderProfileDto,
    ChatStreamItemDto,
    ConversationBranchDto,
    ConversationDto,
    ConversationMode,
    ConversationStateDto,
    ContinueProviderDiscoveryInput,
    CreateProviderConnectionInput,
    CredentialStatusDto,
    CredentialTargetDto,
    EditUserMessageInput,
    GenerationPresetDto,
    GenerationPresetInput,
    GenerateRuntimeTextInput,
    GenerationStartedDto,
    GenerationTargetDto,
    GenerationAttemptProposalDecisionReceiptDto,
    GenerationAttemptProposalExpiryReceiptDto,
    GenerationAttemptProposalListItemDto,
    GenerationAttemptApprovalClientApi,
    RetryableGenerationAttemptDto,
    ImportInspectionDto,
    ImportTicketDto,
    InteractionEffectEventDto,
    InteractionEffectHistoryPageDto,
    InteractionChoiceSelectionReceiptDto,
    InteractionProposalListItemDto,
    InteractionProposalExpiryReceiptDto,
    InteractionReopenSnapshotDto,
    InteractionProposalDecisionReceiptDto,
    ListInteractionEffectHistoryInput,
    ListGenerationAttemptProposalsInput,
    ListInteractionProposalsInput,
    ListPendingContentPackageImportsInput,
    ListReopenInteractionEffectsInput,
    LorepiaClient,
    MessageDto,
    MessageActionGenerationDto,
    ModelRouteDto,
    ModelSyncEventDto,
    ModelSyncJobDto,
    ModelSyncStartedDto,
    NativeCaptureStatusDto,
    PromptCacheControlDto,
    ReviewedPromptSendInput,
    ProviderCatalogDiffDto,
    ProviderCatalogHistoryDto,
    ProviderCatalogImportResultDto,
    ProviderCatalogImportTicketDto,
    ProviderCatalogRollbackPlanDto,
    ProviderCatalogRollbackResultDto,
    ProviderCatalogStatusDto,
    ProviderConnectionDto,
    ProviderDiscoveryApprovalProposalDto,
    ProviderDiscoveryReviewProposalDto,
    ProviderDiscoverySessionDto,
    ProviderProfileDto,
    ProviderTemplateDto,
    ProviderOverviewDto,
    ReasoningControlDto,
    RegenerateAssistantMessageInput,
    ReopenContentPackageImportInput,
    RoomInteractionClientApi,
    RemoveMessageInput,
    RequestPreviewDto,
    SendMessageInput,
    RuntimeTextGenerationDto,
    AppSettingsDto,
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
    EffectiveCapabilityDto,
    ParameterSpecDto,
    UpdateProviderConnectionInput,
    UpsertCapabilityOverrideInput,
    UpsertModelRouteInput,
    SelectContentPackageImportInput,
    SelectContentPackageImportReceiptDto,
    SubmitInteractionChoiceInput,
    DiscardContentPackageImportInput,
} from './contracts';
import { normalizeClientError } from './errors';
import { isInteractionEffectEvent, isMemorySupervisorStatus } from './client-payload-guards';
import type {
    GetPortableRuntimeStateDto,
    PortableRuntimeStateScopeInput,
    PutPortableRuntimeStateInput,
    PutPortableRuntimeStateResultDto,
} from './portable-runtime-state-contracts';

import { LOREPIA_COMMANDS, LOREPIA_EVENTS } from './commands';

export { LOREPIA_COMMANDS, LOREPIA_EVENTS };

type CommandName = (typeof LOREPIA_COMMANDS)[keyof typeof LOREPIA_COMMANDS];

export interface LorepiaTransport {
    invoke(commandName: string, args?: Record<string, unknown>): Promise<unknown>;
    createChatChannel(onMessage: (message: ChatStreamItemDto) => void): unknown;
    listen(eventName: string, onPayload: (payload: unknown) => void): Promise<() => void>;
}

export class TauriTransport implements LorepiaTransport {
    invoke(commandName: string, args?: Record<string, unknown>): Promise<unknown> {
        return invoke(commandName, args);
    }

    createChatChannel(onMessage: (message: ChatStreamItemDto) => void): Channel<ChatStreamItemDto> {
        const channel = new Channel<ChatStreamItemDto>();
        channel.onmessage = onMessage;
        return channel;
    }

    listen(eventName: string, onPayload: (payload: unknown) => void): Promise<() => void> {
        return listen<unknown>(eventName, (event) => onPayload(event.payload));
    }
}

export class LiveLorepiaClient
    implements
        LorepiaClient,
        OrchestrationClientApi,
        OrchestrationDocumentClientApi,
        PromptPresetHistoryClientApi,
        RoomInteractionClientApi,
        GenerationAttemptApprovalClientApi,
        ContentPackageClientApi,
        PersonaClientApi,
        ContentModuleLifecycleClientApi
{
    constructor(private readonly transport: LorepiaTransport = new TauriTransport()) {}

    private async call<Result>(name: CommandName, args?: Record<string, unknown>): Promise<Result> {
        try {
            return (await this.transport.invoke(name, args)) as Result;
        } catch (error: unknown) {
            throw normalizeClientError(error);
        }
    }

    bootstrapSnapshot(): Promise<BootstrapDto> {
        return this.call(LOREPIA_COMMANDS.bootstrap);
    }

    getMemorySupervisorStatus(): Promise<MemorySupervisorStatusDto> {
        return this.call(LOREPIA_COMMANDS.getMemorySupervisorStatus);
    }

    subscribeMemorySupervisorStatus(
        onStatus: (status: MemorySupervisorStatusDto) => void,
    ): Promise<() => void> {
        return this.transport.listen(LOREPIA_EVENTS.memorySupervisorStatus, (payload) => {
            if (isMemorySupervisorStatus(payload)) onStatus(payload);
        });
    }

    listCharacters(): Promise<CharacterDto[]> {
        return this.call(LOREPIA_COMMANDS.listCharacters);
    }

    getCharacter(characterId: string): Promise<CharacterDto> {
        return this.call(LOREPIA_COMMANDS.getCharacter, {
            request: { character_id: characterId },
        });
    }

    getCharacterGreetingCatalog(characterId: string): Promise<CharacterGreetingCatalogDto> {
        return this.call(LOREPIA_COMMANDS.getCharacterGreetingCatalog, {
            request: { character_id: characterId },
        });
    }

    getCharacterRenderProfile(characterId: string): Promise<CharacterRenderProfileDto> {
        return this.call(LOREPIA_COMMANDS.getCharacterRenderProfile, {
            request: { character_id: characterId },
        });
    }

    createPersona(input: CreatePersonaInput): Promise<PersonaDto> {
        return this.call(LOREPIA_COMMANDS.createPersona, { request: input });
    }

    updatePersona(input: UpdatePersonaInput): Promise<PersonaDto> {
        return this.call(LOREPIA_COMMANDS.updatePersona, { request: input });
    }

    getPersona(input: GetPersonaInput): Promise<PersonaDto> {
        return this.call(LOREPIA_COMMANDS.getPersona, { request: input });
    }

    listPersonas(input: ListPersonasInput): Promise<PersonaDto[]> {
        return this.call(LOREPIA_COMMANDS.listPersonas, { request: input });
    }

    listPersonaPage(input: ListPersonaPageInput): Promise<PersonaListPageDto> {
        return this.call(LOREPIA_COMMANDS.listPersonaPage, { request: input });
    }

    deletePersona(input: DeletePersonaInput): Promise<PersonaDeletionReceiptDto> {
        return this.call(LOREPIA_COMMANDS.deletePersona, { request: input });
    }

    getConversationPersonaSelection(
        input: GetConversationPersonaSelectionInput,
    ): Promise<ConversationPersonaSelectionDto> {
        return this.call(LOREPIA_COMMANDS.getConversationPersonaSelection, { request: input });
    }

    selectConversationPersona(
        input: SelectConversationPersonaInput,
    ): Promise<ConversationPersonaSelectionDto> {
        return this.call(LOREPIA_COMMANDS.selectConversationPersona, { request: input });
    }

    clearConversationPersona(
        input: ClearConversationPersonaInput,
    ): Promise<ConversationPersonaSelectionDto> {
        return this.call(LOREPIA_COMMANDS.clearConversationPersona, { request: input });
    }

    resolveAssetDelivery(input: ResolveAssetDeliveryInput): Promise<AssetDeliveryDto> {
        return this.call(LOREPIA_COMMANDS.resolveAssetDelivery, { request: input });
    }

    listInteractionEffects(): Promise<InteractionEffectEventDto[]> {
        return this.call(LOREPIA_COMMANDS.listInteractionEffects);
    }

    acknowledgeInteractionEffect(deliveryId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.acknowledgeInteractionEffect, {
            request: { delivery_id: deliveryId },
        });
    }

    retryInteractionEffect(deliveryId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.retryInteractionEffect, {
            request: { delivery_id: deliveryId },
        });
    }

    decideInteractionProposal(
        input: DecideInteractionProposalInput,
    ): Promise<InteractionProposalDecisionReceiptDto> {
        return this.call(LOREPIA_COMMANDS.decideInteractionProposal, { request: input });
    }

    decideGenerationAttemptProposal(
        input: DecideGenerationAttemptProposalInput,
    ): Promise<GenerationAttemptProposalDecisionReceiptDto> {
        return this.call(LOREPIA_COMMANDS.decideGenerationAttemptProposal, { request: input });
    }

    listInteractionProposals(
        input: ListInteractionProposalsInput,
    ): Promise<InteractionProposalListItemDto[]> {
        return this.call(LOREPIA_COMMANDS.listInteractionProposals, { request: input });
    }

    listGenerationAttemptProposals(
        input: ListGenerationAttemptProposalsInput,
    ): Promise<GenerationAttemptProposalListItemDto[]> {
        return this.call(LOREPIA_COMMANDS.listGenerationAttemptProposals, { request: input });
    }

    listRetryableGenerationAttempts(
        input: ListRetryableGenerationAttemptsInput,
    ): Promise<RetryableGenerationAttemptDto[]> {
        return this.call(LOREPIA_COMMANDS.listRetryableGenerationAttempts, { request: input });
    }

    expireInteractionProposals(
        input: ExpireInteractionProposalsInput,
    ): Promise<InteractionProposalExpiryReceiptDto> {
        return this.call(LOREPIA_COMMANDS.expireInteractionProposals, { request: input });
    }

    expireGenerationAttemptProposals(
        input: ExpireGenerationAttemptProposalsInput,
    ): Promise<GenerationAttemptProposalExpiryReceiptDto> {
        return this.call(LOREPIA_COMMANDS.expireGenerationAttemptProposals, { request: input });
    }

    listInteractionEffectHistory(
        input: ListInteractionEffectHistoryInput,
    ): Promise<InteractionEffectHistoryPageDto> {
        return this.call(LOREPIA_COMMANDS.listInteractionEffectHistory, { request: input });
    }

    listReopenInteractionEffects(
        input: ListReopenInteractionEffectsInput,
    ): Promise<InteractionReopenSnapshotDto> {
        return this.call(LOREPIA_COMMANDS.listReopenInteractionEffects, { request: input });
    }

    submitInteractionChoice(
        input: SubmitInteractionChoiceInput,
    ): Promise<InteractionChoiceSelectionReceiptDto> {
        return this.call(LOREPIA_COMMANDS.submitInteractionChoice, { request: input });
    }

    subscribeInteractionEffects(
        onEffect: (effect: InteractionEffectEventDto) => void,
    ): Promise<() => void> {
        return this.transport.listen(LOREPIA_EVENTS.interactionEffect, (payload) => {
            if (isInteractionEffectEvent(payload)) onEffect(payload);
        });
    }

    listPendingContentPackageImports(
        input: ListPendingContentPackageImportsInput,
    ): Promise<ContentPackageImportReviewDto[]> {
        return this.call(LOREPIA_COMMANDS.listPendingContentPackageImports, {
            request: input,
        });
    }

    pickContentPackageImport(): Promise<ContentPackageInspectionReviewDto | null> {
        return this.call(LOREPIA_COMMANDS.pickContentPackageImport);
    }

    reopenContentPackageImport(
        input: ReopenContentPackageImportInput,
    ): Promise<ContentPackageWorkspaceDto> {
        return this.call(LOREPIA_COMMANDS.reopenContentPackageImport, { request: input });
    }

    selectContentPackageImport(
        input: SelectContentPackageImportInput,
    ): Promise<SelectContentPackageImportReceiptDto> {
        return this.call(LOREPIA_COMMANDS.selectContentPackageImport, { request: input });
    }

    approveContentPackageImport(
        input: ApproveContentPackageImportInput,
    ): Promise<ApproveContentPackageImportReceiptDto> {
        return this.call(LOREPIA_COMMANDS.approveContentPackageImport, { request: input });
    }

    commitContentPackageImport(
        input: CommitContentPackageImportInput,
    ): Promise<CommitContentPackageImportReceiptDto> {
        return this.call(LOREPIA_COMMANDS.commitContentPackageImport, { request: input });
    }

    discardContentPackageImport(
        input: DiscardContentPackageImportInput,
    ): Promise<ContentPackageImportSummaryDto> {
        return this.call(LOREPIA_COMMANDS.discardContentPackageImport, { request: input });
    }

    listCompletedContentPackageExports(
        input: ListCompletedContentPackageExportsInput,
    ): Promise<ContentSourceExportDescriptorDto[]> {
        return this.call(LOREPIA_COMMANDS.listCompletedContentPackageExports, { request: input });
    }

    exportContentSource(
        input: ContentSourceExportInput,
    ): Promise<ContentSourceExportReceiptDto | null> {
        return this.call(LOREPIA_COMMANDS.exportContentSource, { request: input });
    }

    selectImportSource(): Promise<ImportTicketDto | null> {
        return this.call(LOREPIA_COMMANDS.pickImport);
    }

    inspectImport(ticketId: string): Promise<ImportInspectionDto> {
        return this.call(LOREPIA_COMMANDS.inspectImport, {
            request: { ticket_id: ticketId },
        });
    }

    commitImport(inspectionId: string): Promise<CharacterDto> {
        return this.call(LOREPIA_COMMANDS.commitImport, {
            request: { inspection_id: inspectionId },
        });
    }

    discardImport(inspectionId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.discardImport, {
            request: { kind: 'inspection', inspection_id: inspectionId },
        });
    }

    listConversations(characterId: string | null): Promise<ConversationDto[]> {
        if (characterId === null) {
            return this.call(LOREPIA_COMMANDS.listConversations);
        }
        return this.call(LOREPIA_COMMANDS.listConversationsForCharacter, {
            request: { character_id: characterId },
        });
    }

    createConversation(
        characterId: string,
        title: string,
        mode: ConversationMode,
        greeting?: CharacterGreetingSelectionInput,
    ): Promise<ConversationDto> {
        const input: {
            character_id: string;
            title: string;
            mode: ConversationMode;
            greeting?: CharacterGreetingSelectionInput;
        } = { character_id: characterId, title, mode };
        if (greeting !== undefined) input.greeting = greeting;
        return this.call(LOREPIA_COMMANDS.createConversation, {
            input,
        });
    }

    openConversation(characterId: string): Promise<ConversationDto> {
        return this.call(LOREPIA_COMMANDS.openConversation, {
            request: { character_id: characterId },
        });
    }

    openExistingConversation(conversationId: string): Promise<ConversationDto> {
        return this.call(LOREPIA_COMMANDS.openExistingConversation, {
            request: { conversation_id: conversationId },
        });
    }

    getConversation(conversationId: string): Promise<ConversationDto> {
        return this.call(LOREPIA_COMMANDS.getConversation, {
            request: { conversation_id: conversationId },
        });
    }

    getConversationState(conversationId: string): Promise<ConversationStateDto> {
        return this.call(LOREPIA_COMMANDS.getConversationState, {
            request: { conversation_id: conversationId },
        });
    }

    listBranches(conversationId: string): Promise<ConversationBranchDto[]> {
        return this.call(LOREPIA_COMMANDS.listBranches, {
            request: { conversation_id: conversationId },
        });
    }

    createBranch(
        conversationId: string,
        fromMessageId: string | null,
        title: string | null,
    ): Promise<ConversationBranchDto> {
        return this.call(LOREPIA_COMMANDS.createBranch, {
            input: {
                conversation_id: conversationId,
                from_message_id: fromMessageId,
                title,
            },
        });
    }

    selectBranch(conversationId: string, branchId: string): Promise<ConversationStateDto> {
        return this.call(LOREPIA_COMMANDS.selectBranch, {
            input: { conversation_id: conversationId, branch_id: branchId },
        });
    }

    setConversationMode(
        conversationId: string,
        mode: ConversationMode,
    ): Promise<ConversationStateDto> {
        return this.call(LOREPIA_COMMANDS.setConversationMode, {
            input: { conversation_id: conversationId, mode },
        });
    }

    listBranchMessages(branchId: string): Promise<MessageDto[]> {
        return this.call(LOREPIA_COMMANDS.listBranchMessages, {
            request: { branch_id: branchId },
        });
    }

    listMessages(conversationId: string): Promise<MessageDto[]> {
        return this.call(LOREPIA_COMMANDS.listMessages, {
            request: { conversation_id: conversationId },
        });
    }

    generateRuntimeText(input: GenerateRuntimeTextInput): Promise<RuntimeTextGenerationDto> {
        return this.call(LOREPIA_COMMANDS.generateRuntimeText, { input });
    }

    cancelRuntimeText(requestId: string): Promise<boolean> {
        return this.call(LOREPIA_COMMANDS.cancelRuntimeText, {
            request: { request_id: requestId },
        });
    }

    getPortableRuntimeState(
        scope: PortableRuntimeStateScopeInput,
    ): Promise<GetPortableRuntimeStateDto> {
        return this.call(LOREPIA_COMMANDS.getPortableRuntimeState, { request: { scope } });
    }

    putPortableRuntimeState(
        input: PutPortableRuntimeStateInput,
    ): Promise<PutPortableRuntimeStateResultDto> {
        return this.call(LOREPIA_COMMANDS.putPortableRuntimeState, { request: input });
    }

    sendMessage(
        input: SendMessageInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<GenerationStartedDto> {
        const onEvent = this.transport.createChatChannel(onItem);
        return this.call(LOREPIA_COMMANDS.sendMessage, { input, streamId, onEvent });
    }

    sendReviewedPrompt(
        input: ReviewedPromptSendInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<GenerationStartedDto> {
        const onEvent = this.transport.createChatChannel(onItem);
        return this.call(LOREPIA_COMMANDS.sendReviewedPrompt, { input, streamId, onEvent });
    }

    editUserMessage(
        input: EditUserMessageInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<MessageActionGenerationDto> {
        const onEvent = this.transport.createChatChannel(onItem);
        return this.call(LOREPIA_COMMANDS.editUserMessage, { input, streamId, onEvent });
    }

    regenerateAssistantMessage(
        input: RegenerateAssistantMessageInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<MessageActionGenerationDto> {
        const onEvent = this.transport.createChatChannel(onItem);
        return this.call(LOREPIA_COMMANDS.regenerateAssistantMessage, {
            input,
            streamId,
            onEvent,
        });
    }

    removeMessageFromBranch(input: RemoveMessageInput): Promise<ConversationBranchDto> {
        return this.call(LOREPIA_COMMANDS.removeMessageFromBranch, { input });
    }

    cancelGeneration(generationId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.cancelGeneration, {
            request: { generation_id: generationId },
        });
    }

    subscribeGeneration(
        generationId: string,
        conversationId: string,
        branchId: string,
        sequenceBaseline: number,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<void> {
        const onEvent = this.transport.createChatChannel(onItem);
        return this.call(LOREPIA_COMMANDS.subscribeGeneration, {
            request: {
                generation_id: generationId,
                conversation_id: conversationId,
                branch_id: branchId,
                sequence_baseline: sequenceBaseline,
            },
            streamId,
            onEvent,
        });
    }

    disposeChatStream(streamId: string): Promise<boolean> {
        return this.call(LOREPIA_COMMANDS.disposeChatStream, {
            request: { stream_id: streamId },
        });
    }

    getProviderOverview(): Promise<ProviderOverviewDto> {
        return this.call(LOREPIA_COMMANDS.getProviderOverview);
    }

    getSettings(): Promise<AppSettingsDto> {
        return this.call(LOREPIA_COMMANDS.getSettings);
    }

    updateSettings(settings: AppSettingsDto): Promise<AppSettingsDto> {
        return this.call(LOREPIA_COMMANDS.updateSettings, { request: { settings } });
    }

    selectGenerationTarget(target: GenerationTargetDto | null): Promise<AppSettingsDto> {
        return this.call(LOREPIA_COMMANDS.selectGenerationTarget, { request: { target } });
    }

    listProviderTemplates(): Promise<ProviderTemplateDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderTemplates);
    }

    listProviderConnections(): Promise<ProviderConnectionDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderConnections);
    }

    createProviderConnection(input: CreateProviderConnectionInput): Promise<ProviderConnectionDto> {
        return this.call(LOREPIA_COMMANDS.createProviderConnection, {
            request: { input },
        });
    }

    upsertProviderConnection(input: UpdateProviderConnectionInput): Promise<ProviderConnectionDto> {
        return this.call(LOREPIA_COMMANDS.upsertProviderConnection, {
            request: { input },
        });
    }

    deleteProviderConnection(connectionId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.deleteProviderConnection, {
            request: { connection_id: connectionId },
        });
    }

    listProviderProfiles(): Promise<ProviderProfileDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderProfiles);
    }

    listModelRoutes(connectionId: string): Promise<ModelRouteDto[]> {
        return this.call(LOREPIA_COMMANDS.listModelRoutes, {
            request: { connection_id: connectionId },
        });
    }

    upsertModelRoute(input: UpsertModelRouteInput): Promise<ModelRouteDto> {
        return this.call(LOREPIA_COMMANDS.upsertModelRoute, { request: { input } });
    }

    deleteModelRoute(routeId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.deleteModelRoute, {
            request: { model_route_id: routeId },
        });
    }

    listCapabilityObservations(modelRouteId: string): Promise<CapabilityObservationDto[]> {
        return this.call(LOREPIA_COMMANDS.listCapabilityObservations, {
            request: { model_route_id: modelRouteId },
        });
    }

    effectiveCapability(
        modelRouteId: string,
        key: CapabilityKeyInput,
    ): Promise<EffectiveCapabilityDto | null> {
        return this.call(LOREPIA_COMMANDS.effectiveCapability, {
            request: { model_route_id: modelRouteId, key },
        });
    }

    effectiveParameterSpecs(modelRouteId: string): Promise<ParameterSpecDto[]> {
        return this.call(LOREPIA_COMMANDS.effectiveParameterSpecs, {
            request: { model_route_id: modelRouteId },
        });
    }

    upsertUserCapabilityOverride(
        input: UpsertCapabilityOverrideInput,
    ): Promise<CapabilityObservationDto> {
        return this.call(LOREPIA_COMMANDS.upsertUserCapabilityOverride, {
            request: { input },
        });
    }

    deleteUserCapabilityOverride(modelRouteId: string, observationId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.deleteUserCapabilityOverride, {
            request: {
                model_route_id: modelRouteId,
                observation_id: observationId,
            },
        });
    }

    listGenerationPresets(routeId: string): Promise<GenerationPresetDto[]> {
        return this.call(LOREPIA_COMMANDS.listGenerationPresets, {
            request: { model_route_id: routeId },
        });
    }

    upsertGenerationPreset(input: GenerationPresetInput): Promise<GenerationPresetDto> {
        return this.call(LOREPIA_COMMANDS.upsertGenerationPreset, {
            request: { input },
        });
    }

    deleteGenerationPreset(presetId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.deleteGenerationPreset, {
            request: { generation_preset_id: presetId },
        });
    }

    validateGenerationPresetCandidate(input: GenerationPresetInput): Promise<void> {
        return this.call(LOREPIA_COMMANDS.validateGenerationPresetCandidate, {
            request: { input },
        });
    }

    renderReasoningControlForPreset(input: GenerationPresetInput): Promise<ReasoningControlDto> {
        return this.call(LOREPIA_COMMANDS.renderReasoningControlForPreset, {
            request: { input },
        });
    }

    renderPromptCacheControlForPreset(
        input: GenerationPresetInput,
    ): Promise<PromptCacheControlDto> {
        return this.call(LOREPIA_COMMANDS.renderPromptCacheControlForPreset, {
            request: { input },
        });
    }

    previewProviderRequestCandidate(input: GenerationPresetInput): Promise<RequestPreviewDto> {
        return this.call(LOREPIA_COMMANDS.previewProviderRequestCandidate, {
            request: { input },
        });
    }

    credentialStatus(target: CredentialTargetDto): Promise<CredentialStatusDto> {
        return this.call(LOREPIA_COMMANDS.credentialStatus, { request: { target } });
    }

    captureCredential(target: CredentialTargetDto): Promise<NativeCaptureStatusDto> {
        return this.call(LOREPIA_COMMANDS.captureCredential, { request: { target } });
    }

    deleteCredential(target: CredentialTargetDto): Promise<void> {
        return this.call(LOREPIA_COMMANDS.deleteCredential, { request: { target } });
    }

    previewProviderRequest(target: GenerationTargetDto): Promise<RequestPreviewDto> {
        return this.call(LOREPIA_COMMANDS.previewProviderRequest, {
            request: { target },
        });
    }

    startProviderModelSync(connectionId: string): Promise<ModelSyncStartedDto> {
        return this.call(LOREPIA_COMMANDS.startProviderModelSync, {
            request: { connection_id: connectionId },
        });
    }

    getProviderModelSync(jobId: string): Promise<ModelSyncJobDto> {
        return this.call(LOREPIA_COMMANDS.getProviderModelSync, {
            request: { job_id: jobId },
        });
    }

    listProviderModelSyncs(connectionId: string, limit: number): Promise<ModelSyncJobDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderModelSyncs, {
            request: { connection_id: connectionId, limit },
        });
    }

    approveProviderModelSync(jobId: string, reviewSha256: string): Promise<ModelSyncJobDto> {
        return this.call(LOREPIA_COMMANDS.approveProviderModelSync, {
            request: { job_id: jobId, review_sha256: reviewSha256 },
        });
    }

    cancelProviderModelSync(jobId: string): Promise<ModelSyncJobDto> {
        return this.call(LOREPIA_COMMANDS.cancelProviderModelSync, {
            request: { job_id: jobId },
        });
    }

    pollProviderModelSyncEvents(jobId: string, limit: number): Promise<ModelSyncEventDto[]> {
        return this.call(LOREPIA_COMMANDS.pollProviderModelSyncEvents, {
            request: { job_id: jobId, limit },
        });
    }

    ackProviderModelSyncEvent(jobId: string, sequence: number): Promise<boolean> {
        return this.call(LOREPIA_COMMANDS.ackProviderModelSyncEvent, {
            request: { job_id: jobId, sequence },
        });
    }

    beginProviderDiscovery(
        input: BeginProviderDiscoveryInput,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.beginProviderDiscovery, {
            request: { input },
        });
    }

    beginProviderDiscoveryCurl(
        input: BeginProviderDiscoveryCurlInput,
    ): Promise<CapturedProviderDiscoveryDto> {
        return this.call(LOREPIA_COMMANDS.beginProviderDiscoveryCurl, {
            request: { input },
        });
    }

    listProviderDiscoveries(limit: number): Promise<ProviderDiscoverySessionDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderDiscoveries, {
            request: { limit },
        });
    }

    getProviderDiscovery(sessionId: string): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.getProviderDiscovery, {
            request: { session_id: sessionId },
        });
    }

    listProviderDiscoveryCandidates(sessionId: string): Promise<DiscoveryCandidateDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderDiscoveryCandidates, {
            request: { session_id: sessionId },
        });
    }

    listProviderDiscoveryEvidence(sessionId: string): Promise<DiscoveryEvidenceDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderDiscoveryEvidence, {
            request: { session_id: sessionId },
        });
    }

    listProviderDiscoveryApprovals(sessionId: string): Promise<DiscoveryApprovalRecordDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderDiscoveryApprovals, {
            request: { session_id: sessionId },
        });
    }

    getProviderDiscoveryReview(sessionId: string): Promise<DiscoveryReviewDto | null> {
        return this.call(LOREPIA_COMMANDS.getProviderDiscoveryReview, {
            request: { session_id: sessionId },
        });
    }

    getProviderDiscoveryApprovalProposal(
        sessionId: string,
    ): Promise<ProviderDiscoveryApprovalProposalDto | null> {
        return this.call(LOREPIA_COMMANDS.getProviderDiscoveryApprovalProposal, {
            request: { session_id: sessionId },
        });
    }

    getProviderDiscoveryReviewProposal(
        sessionId: string,
    ): Promise<ProviderDiscoveryReviewProposalDto | null> {
        return this.call(LOREPIA_COMMANDS.getProviderDiscoveryReviewProposal, {
            request: { session_id: sessionId },
        });
    }

    getProviderDiscoveryAssistantResumeBoundary(
        sessionId: string,
    ): Promise<DiscoveryAssistantResumeBoundaryDto | null> {
        return this.call(LOREPIA_COMMANDS.getProviderDiscoveryAssistantResumeBoundary, {
            request: { session_id: sessionId },
        });
    }

    runProviderDiscoveryAssistantTurn(sessionId: string): Promise<DiscoveryAssistantHostActionDto> {
        return this.call(LOREPIA_COMMANDS.runProviderDiscoveryAssistantTurn, {
            request: { session_id: sessionId },
        });
    }

    resumeProviderDiscoveryAssistantCoreHostAction(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.resumeProviderDiscoveryAssistantCoreHostAction, {
            request: { session_id: sessionId },
        });
    }

    approveProviderDiscoveryAssistantRetry(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.approveProviderDiscoveryAssistantRetry, {
            request: { session_id: sessionId },
        });
    }

    requestProviderDiscoveryAssistantRevision(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.requestProviderDiscoveryAssistantRevision, {
            request: { session_id: sessionId },
        });
    }

    acceptProviderDiscoveryAssistantDraft(sessionId: string): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.acceptProviderDiscoveryAssistantDraft, {
            request: { session_id: sessionId },
        });
    }

    recordProviderDiscoveryAssistantFailure(
        sessionId: string,
        kind: DiscoveryAssistantFailureKindInput,
        retryable: boolean,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.recordProviderDiscoveryAssistantFailure, {
            request: { session_id: sessionId, kind, retryable },
        });
    }

    interruptProviderDiscoveryAssistant(
        sessionId: string,
        outcome: DiscoveryAssistantInterruptionOutcomeInput,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.interruptProviderDiscoveryAssistant, {
            request: { session_id: sessionId, outcome },
        });
    }

    restartProviderDiscoveryAssistantAfterInterruption(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.restartProviderDiscoveryAssistantAfterInterruption, {
            request: { session_id: sessionId },
        });
    }

    continueProviderDiscovery(
        input: ContinueProviderDiscoveryInput,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.continueProviderDiscovery, {
            request: { input },
        });
    }

    supplyProviderDiscoveryDocumentEvidence(
        sessionId: string,
        expectedRevision: number,
        documentUrl: string,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.supplyProviderDiscoveryDocumentEvidence, {
            request: {
                session_id: sessionId,
                expected_revision: expectedRevision,
                document_url: documentUrl,
            },
        });
    }

    supplyProviderDiscoveryCurlEvidence(
        sessionId: string,
        expectedRevision: number,
    ): Promise<CapturedProviderDiscoveryDto> {
        return this.call(LOREPIA_COMMANDS.supplyProviderDiscoveryCurlEvidence, {
            request: {
                session_id: sessionId,
                expected_revision: expectedRevision,
            },
        });
    }

    cancelProviderDiscovery(
        sessionId: string,
        expectedRevision: number,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.cancelProviderDiscovery, {
            request: {
                session_id: sessionId,
                expected_revision: expectedRevision,
            },
        });
    }

    commitProviderDiscovery(sessionId: string): Promise<ProviderConnectionDto> {
        return this.call(LOREPIA_COMMANDS.commitProviderDiscovery, {
            request: { session_id: sessionId },
        });
    }

    pollProviderDiscoveryEvents(limit: number): Promise<DiscoveryOutboxEventDto[]> {
        return this.call(LOREPIA_COMMANDS.pollProviderDiscoveryEvents, {
            request: { limit },
        });
    }

    pollProviderDiscoveryEventsForSession(
        sessionId: string,
        limit: number,
    ): Promise<DiscoveryOutboxEventDto[]> {
        return this.call(LOREPIA_COMMANDS.pollProviderDiscoveryEventsForSession, {
            request: {
                session_id: sessionId,
                limit,
            },
        });
    }

    ackProviderDiscoveryEvent(eventId: string): Promise<boolean> {
        return this.call(LOREPIA_COMMANDS.ackProviderDiscoveryEvent, {
            request: { event_id: eventId },
        });
    }

    recoverProviderDiscovery(): Promise<DiscoveryRecoveryResultDto[]> {
        return this.call(LOREPIA_COMMANDS.recoverProviderDiscovery);
    }

    listProviderDiscoveryCompensationSteps(
        commitAttemptId: string,
    ): Promise<DiscoveryCompensationRecordDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderDiscoveryCompensationSteps, {
            request: { commit_attempt_id: commitAttemptId },
        });
    }

    continueProviderDiscoveryCompensation(sessionId: string): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.continueProviderDiscoveryCompensation, {
            request: { session_id: sessionId },
        });
    }

    resumeProviderDiscoveryCompensation(sessionId: string): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.resumeProviderDiscoveryCompensation, {
            request: { session_id: sessionId },
        });
    }

    pickProviderCatalogImport(): Promise<ProviderCatalogImportTicketDto | null> {
        return this.call(LOREPIA_COMMANDS.pickProviderCatalogImport);
    }

    activateProviderCatalogImport(ticketId: string): Promise<ProviderCatalogImportResultDto> {
        return this.call(LOREPIA_COMMANDS.activateProviderCatalogImport, {
            request: { ticket_id: ticketId },
        });
    }

    discardProviderCatalogImport(ticketId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.discardProviderCatalogImport, {
            request: { ticket_id: ticketId },
        });
    }

    providerCatalogStatus(): Promise<ProviderCatalogStatusDto> {
        return this.call(LOREPIA_COMMANDS.providerCatalogStatus);
    }

    providerCatalogHistory(
        limit: number,
        beforeRevision: number | null,
        beforeStateVersion: number | null,
    ): Promise<ProviderCatalogHistoryDto> {
        return this.call(LOREPIA_COMMANDS.providerCatalogHistory, {
            request: {
                limit,
                before_revision: beforeRevision,
                before_state_version: beforeStateVersion,
            },
        });
    }

    diffProviderCatalogRevisions(
        fromRevision: number,
        toRevision: number,
    ): Promise<ProviderCatalogDiffDto> {
        return this.call(LOREPIA_COMMANDS.diffProviderCatalogRevisions, {
            request: { from_revision: fromRevision, to_revision: toRevision },
        });
    }

    prepareProviderCatalogRollback(
        targetRevision: number,
    ): Promise<ProviderCatalogRollbackPlanDto> {
        return this.call(LOREPIA_COMMANDS.prepareProviderCatalogRollback, {
            request: { target_revision: targetRevision },
        });
    }

    activateProviderCatalogRollback(
        plan: ProviderCatalogRollbackPlanDto,
    ): Promise<ProviderCatalogRollbackResultDto> {
        return this.call(LOREPIA_COMMANDS.activateProviderCatalogRollback, {
            request: { plan },
        });
    }

    getOrchestrationWorkspace(
        conversationId: string,
        branchId: string,
    ): Promise<OrchestrationWorkspaceSnapshotDto> {
        return this.call(LOREPIA_COMMANDS.getOrchestrationWorkspace, {
            request: {
                conversation_id: conversationId,
                branch_id: branchId,
            },
        });
    }

    saveRoomOrchestrationConfig(
        input: SaveRoomOrchestrationConfigInput,
    ): Promise<SaveRoomOrchestrationConfigResult> {
        return this.call(LOREPIA_COMMANDS.saveRoomOrchestrationConfig, { request: input });
    }

    resolvePromptPreview(input: PromptPlanRequestInput): Promise<PromptPlanPreviewDto> {
        return this.call(LOREPIA_COMMANDS.resolvePromptPreview, { request: input });
    }

    explainPromptPlan(input: ExplainPromptPlanInput): Promise<PromptResolutionTraceDto> {
        return this.call(LOREPIA_COMMANDS.explainPromptPlan, { request: input });
    }

    upsertPromptPreset(
        input: UpsertPromptPresetInput,
    ): Promise<RevisionedDto<PromptPresetSummaryDto>> {
        return this.call(LOREPIA_COMMANDS.upsertPromptPreset, { request: input });
    }

    getPromptPreset(input: GetPromptPresetInput): Promise<RevisionedDto<PromptPresetSummaryDto>> {
        return this.call(LOREPIA_COMMANDS.getPromptPreset, { request: input });
    }

    getEditablePromptPreset(
        input: GetPromptPresetInput,
    ): Promise<RevisionedDto<CreatorPromptPresetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.getEditablePromptPreset, { request: input });
    }

    listPromptPresets(): Promise<RevisionedDto<PromptPresetSummaryDto>[]> {
        return this.call(LOREPIA_COMMANDS.listPromptPresets);
    }

    listPromptPresetRevisions(
        input: ListPromptPresetRevisionsInput,
    ): Promise<PromptPresetRevisionListDto> {
        return this.call(LOREPIA_COMMANDS.listPromptPresetRevisions, { request: input });
    }

    diffPromptPresetRevisions(
        input: DiffPromptPresetRevisionsInput,
    ): Promise<PromptPresetRevisionDiffDto> {
        return this.call(LOREPIA_COMMANDS.diffPromptPresetRevisions, { request: input });
    }

    reviewPromptPresetRollback(
        input: ReviewPromptPresetRollbackInput,
    ): Promise<PromptPresetRollbackReviewDto> {
        return this.call(LOREPIA_COMMANDS.reviewPromptPresetRollback, { request: input });
    }

    applyPromptPresetRollback(
        input: ApplyPromptPresetRollbackInput,
    ): Promise<PromptPresetRollbackReceiptDto> {
        return this.call(LOREPIA_COMMANDS.applyPromptPresetRollback, { request: input });
    }

    deletePromptPreset(
        input: DeletePromptPresetInput,
    ): Promise<RevisionedDto<PromptPresetSummaryDto>> {
        return this.call(LOREPIA_COMMANDS.deletePromptPreset, { request: input });
    }

    reorderPromptBlocks(input: ReorderPromptBlocksInput): Promise<ReorderPromptBlocksResult> {
        return this.call(LOREPIA_COMMANDS.reorderPromptBlocks, { request: input });
    }

    listTaskProfiles(): Promise<RevisionedDto<TaskProfileDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listTaskProfiles);
    }

    upsertTaskProfile(
        input: UpsertTaskProfileInput,
    ): Promise<RevisionedDto<TaskProfileDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.upsertTaskProfile, { request: input });
    }

    deleteTaskProfile(
        input: DeleteTaskProfileInput,
    ): Promise<RevisionedDto<TaskProfileDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.deleteTaskProfile, { request: input });
    }

    listMemoryProfiles(): Promise<RevisionedDto<CreatorMemoryProfileDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listMemoryProfiles);
    }

    getMemoryProfile(
        input: GetMemoryProfileInput,
    ): Promise<RevisionedDto<CreatorMemoryProfileDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.getMemoryProfile, { request: input });
    }

    upsertMemoryProfile(
        input: UpsertMemoryProfileInput,
    ): Promise<RevisionedDto<CreatorMemoryProfileDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.upsertMemoryProfile, { request: input });
    }

    deleteMemoryProfile(
        input: DeleteMemoryProfileInput,
    ): Promise<RevisionedDto<CreatorMemoryProfileDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.deleteMemoryProfile, { request: input });
    }

    listKnowledgeBooks(): Promise<RevisionedDto<CreatorKnowledgeBookDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listKnowledgeBooks);
    }

    getKnowledgeBook(
        input: GetKnowledgeBookInput,
    ): Promise<RevisionedDto<CreatorKnowledgeBookDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.getKnowledgeBook, { request: input });
    }

    upsertKnowledgeBook(
        input: UpsertKnowledgeBookInput,
    ): Promise<RevisionedDto<CreatorKnowledgeBookDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.upsertKnowledgeBook, { request: input });
    }

    deleteKnowledgeBook(
        input: DeleteKnowledgeBookInput,
    ): Promise<RevisionedDto<CreatorKnowledgeBookDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.deleteKnowledgeBook, { request: input });
    }

    listTransformSets(): Promise<RevisionedDto<CreatorTransformSetDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listTransformSets);
    }

    getTransformSet(
        input: GetTransformSetInput,
    ): Promise<RevisionedDto<CreatorTransformSetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.getTransformSet, { request: input });
    }

    upsertTransformSet(
        input: UpsertTransformSetInput,
    ): Promise<RevisionedDto<CreatorTransformSetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.upsertTransformSet, { request: input });
    }

    deleteTransformSet(
        input: DeleteTransformSetInput,
    ): Promise<RevisionedDto<CreatorTransformSetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.deleteTransformSet, { request: input });
    }

    listInteractionRuleSets(): Promise<RevisionedDto<CreatorInteractionRuleSetDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listInteractionRuleSets);
    }

    getInteractionRuleSet(
        input: GetInteractionRuleSetInput,
    ): Promise<RevisionedDto<CreatorInteractionRuleSetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.getInteractionRuleSet, { request: input });
    }

    upsertInteractionRuleSet(
        input: UpsertInteractionRuleSetInput,
    ): Promise<RevisionedDto<CreatorInteractionRuleSetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.upsertInteractionRuleSet, { request: input });
    }

    deleteInteractionRuleSet(
        input: DeleteInteractionRuleSetInput,
    ): Promise<RevisionedDto<CreatorInteractionRuleSetDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.deleteInteractionRuleSet, { request: input });
    }

    listContentModules(): Promise<RevisionedDto<CreatorContentModuleDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listContentModules);
    }

    getContentModule(
        input: GetContentModuleInput,
    ): Promise<RevisionedDto<CreatorContentModuleDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.getContentModule, { request: input });
    }

    upsertContentModule(
        input: UpsertContentModuleInput,
    ): Promise<RevisionedDto<CreatorContentModuleDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.upsertContentModule, { request: input });
    }

    deleteContentModule(
        input: DeleteContentModuleInput,
    ): Promise<RevisionedDto<CreatorContentModuleDocumentDto>> {
        return this.call(LOREPIA_COMMANDS.deleteContentModule, { request: input });
    }

    deleteMemoryRecord(input: DeleteMemoryRecordRequest): Promise<void> {
        return this.call(LOREPIA_COMMANDS.deleteMemoryRecord, { request: input });
    }

    getMemoryRecord(input: GetMemoryRecordInput): Promise<MemoryRecordDto> {
        return this.call(LOREPIA_COMMANDS.getMemoryRecord, { request: input });
    }

    patchMemoryRecord(input: PatchMemoryRecordRequest): Promise<MemoryRecordDto> {
        return this.call(LOREPIA_COMMANDS.patchMemoryRecord, { request: input });
    }

    setMemoryRecordExclusion(input: SetMemoryRecordExclusionRequest): Promise<MemoryRecordDto> {
        return this.call(LOREPIA_COMMANDS.setMemoryRecordExclusion, { request: input });
    }

    listPromptPresetBindings(
        input: ListPromptPresetBindingsInput,
    ): Promise<RevisionedDto<PromptPresetBindingDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listPromptPresetBindings, { request: input });
    }

    listMemoryRecords(input: ListMemoryRecordsInput): Promise<MemoryRecordListResultDto> {
        return this.call(LOREPIA_COMMANDS.listMemoryRecords, { request: input });
    }

    async simulateKnowledge(input: SimulateKnowledgeRequest): Promise<KnowledgeSimulationDto> {
        const result = await this.simulateKnowledgeActivation({
            knowledge_book_id: input.knowledge_book_id,
            sample_texts: [input.sample_text],
            manual_entry_ids: [],
            semantic_scores: [],
            variables: input.variables,
            supported_capabilities: [],
            token_estimates: [],
            activation_seed: 0,
        });
        const selectedById = new Map(result.selected.map((entry) => [entry.entry_id, entry]));
        return {
            sample_text: input.sample_text,
            entries: result.evidence.map((evidence) => {
                const selected = selectedById.get(evidence.entry_id);
                const semantic = evidence.reasons.find((reason) => reason.kind === 'semantic');
                return {
                    id: evidence.entry_id,
                    source_kind: 'knowledge' as const,
                    title: evidence.entry_id,
                    selected: evidence.selected,
                    reason:
                        evidence.exclusion_reason ??
                        (evidence.reasons.map((reason) => reason.kind).join(', ') ||
                            'not_selected'),
                    score:
                        semantic?.kind === 'semantic'
                            ? semantic.score_millionths / 1_000_000
                            : null,
                    estimated_tokens: evidence.estimated_tokens,
                    placement: selected?.placement ?? null,
                };
            }),
            total_estimated_tokens: result.used_tokens,
            truncated: result.truncated,
        };
    }

    previewTransformRule(input: PreviewTransformRuleInput): Promise<TransformRulePreviewDto> {
        return this.call(LOREPIA_COMMANDS.previewTransformRule, { request: input });
    }

    async previewTransform(input: PreviewTransformRequest): Promise<TransformPreviewDto> {
        const result = await this.previewTransformRule({
            transform_set_id: input.transform_set_id,
            transform_rule_id: input.rule_id,
            sample_text: input.sample_text,
            variables: input.variables,
            supported_capabilities: [],
            approved_import_source_ids: [],
            allow_resolved_prompt: false,
        });
        return {
            transform_set_id: input.transform_set_id,
            rule_id: input.rule_id,
            phase: result.phase,
            input: result.original,
            output: result.output,
            changed: result.changed,
            rendering: result.rendering,
            used_original:
                result.error !== null ||
                result.reports.some((report) => report.status === 'failed'),
            diagnostics: [
                ...(result.error === null ? [] : [result.error.message]),
                ...result.reports.flatMap((report) =>
                    report.trace.error === null ? [] : [report.trace.error],
                ),
            ],
            reports: result.reports,
            diff: result.diff,
            error: result.error,
            truncated: result.truncated,
        };
    }

    listInterruptedMemoryJobs(
        input: ListInterruptedMemoryJobsInput,
    ): Promise<InterruptedMemoryJobDto[]> {
        return this.call(LOREPIA_COMMANDS.listInterruptedMemoryJobs, { request: input });
    }

    retryInterruptedMemoryJob(
        input: RetryInterruptedMemoryJobInput,
    ): Promise<MemoryJobRetryReceiptDto> {
        return this.call(LOREPIA_COMMANDS.retryInterruptedMemoryJob, { request: input });
    }

    listRetryableMemoryQueryEmbeddings(
        input: ListRetryableMemoryQueryEmbeddingsInput,
    ): Promise<MemoryQueryEmbeddingRetryCandidateDto[]> {
        return this.call(LOREPIA_COMMANDS.listRetryableMemoryQueryEmbeddings, { request: input });
    }

    retryMemoryQueryEmbedding(
        input: RetryMemoryQueryEmbeddingInput,
    ): Promise<MemoryQueryEmbeddingRetryCandidateDto> {
        return this.call(LOREPIA_COMMANDS.retryMemoryQueryEmbedding, { request: input });
    }

    simulateKnowledgeActivation(
        input: SimulateKnowledgeActivationInput,
    ): Promise<KnowledgeActivationResultDto> {
        return this.call(LOREPIA_COMMANDS.simulateKnowledgeActivation, { request: input });
    }

    listContentModuleBindings(
        input: ListContentModuleBindingsInput,
    ): Promise<RevisionedDto<ModuleBindingDocumentDto>[]> {
        return this.call(LOREPIA_COMMANDS.listContentModuleBindings, { request: input });
    }

    listContentModuleRevisions(
        input: ListContentModuleRevisionsInput,
    ): Promise<ContentModuleRevisionListResultDto> {
        return this.call(LOREPIA_COMMANDS.listContentModuleRevisions, { request: input });
    }

    diffContentModuleRevisionDocuments(
        input: DiffContentModuleRevisionsInput,
    ): Promise<ContentModuleRevisionDiffDocumentDto> {
        return this.call(LOREPIA_COMMANDS.diffContentModuleRevisions, { request: input });
    }

    evaluateContentModuleShare(
        input: EvaluateContentModuleShareInput,
    ): Promise<ContentShareGateDto> {
        return this.call(LOREPIA_COMMANDS.evaluateContentModuleShare, { request: input });
    }

    listContentModuleLifecycleCandidates(
        input: ListContentModuleLifecycleCandidatesInput,
    ): Promise<ContentModuleLifecycleCandidateListDto> {
        return this.call(LOREPIA_COMMANDS.listContentModuleLifecycleCandidates, { request: input });
    }

    listContentModuleLifecycleBindings(
        input: ListContentModuleLifecycleBindingsInput,
    ): Promise<ContentModuleLifecycleBindingListDto> {
        return this.call(LOREPIA_COMMANDS.listContentModuleLifecycleBindings, { request: input });
    }

    reviewContentModuleActivation(
        input: ReviewContentModuleActivationInput,
    ): Promise<ContentModuleActivationReviewPresentationDto> {
        return this.call(LOREPIA_COMMANDS.reviewContentModuleActivation, { request: input });
    }

    resolveContentModuleActivation(
        input: ResolveContentModuleActivationInput,
    ): Promise<ContentModuleActivationPlanDto> {
        return this.call(LOREPIA_COMMANDS.resolveContentModuleActivation, { request: input });
    }

    activateContentModule(
        input: ActivateContentModuleInput,
    ): Promise<ContentModuleActivationReceiptDto> {
        return this.call(LOREPIA_COMMANDS.activateContentModule, { request: input });
    }

    reviewContentModuleDeactivation(
        input: ReviewContentModuleDeactivationInput,
    ): Promise<ContentModuleDeactivationReviewDto> {
        return this.call(LOREPIA_COMMANDS.reviewContentModuleDeactivation, { request: input });
    }

    deactivateContentModule(
        input: DeactivateContentModuleInput,
    ): Promise<ContentModuleDeactivationReceiptDto> {
        return this.call(LOREPIA_COMMANDS.deactivateContentModule, { request: input });
    }

    reviewContentModuleRollback(
        input: ReviewContentModuleRollbackInput,
    ): Promise<ContentModuleRollbackReviewPresentationDto> {
        return this.call(LOREPIA_COMMANDS.reviewContentModuleRollback, { request: input });
    }

    resolveContentModuleRollback(
        input: ResolveContentModuleRollbackInput,
    ): Promise<ContentModuleRollbackPlanDto> {
        return this.call(LOREPIA_COMMANDS.resolveContentModuleRollback, { request: input });
    }

    applyContentModuleRollback(
        input: ApplyContentModuleRollbackInput,
    ): Promise<ContentModuleActivationReceiptDto> {
        return this.call(LOREPIA_COMMANDS.applyContentModuleRollback, { request: input });
    }
}

export function createLiveLorepiaClient(): LorepiaClient &
    OrchestrationDocumentClientApi &
    PromptPresetHistoryClientApi &
    RoomInteractionClientApi &
    GenerationAttemptApprovalClientApi &
    ContentPackageClientApi &
    PersonaClientApi &
    ContentModuleLifecycleClientApi {
    return new LiveLorepiaClient();
}
