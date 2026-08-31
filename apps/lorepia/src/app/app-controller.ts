import { get, writable, type Readable } from 'svelte/store';

import {
    SUPPORTED_CHAT_EVENT_VERSION,
    SUPPORTED_CORE_API_VERSION,
    SUPPORTED_SHELL_API_VERSION,
    type BeginProviderDiscoveryCurlInput,
    type BeginProviderDiscoveryInput,
    type BootstrapDto,
    type CapabilityKeyInput,
    type CharacterDto,
    type ContinueProviderDiscoveryActionInput,
    type ConversationDto,
    type ConversationMode,
    type CreateProviderConnectionInput,
    type CredentialTargetDto,
    type GenerationPresetInput,
    type DiscoveryAssistantFailureKindInput,
    type DiscoveryAssistantInterruptionOutcomeInput,
    type GenerationPresetDto,
    type GenerationSelectionInput,
    type GenerationTargetDto,
    type LorepiaClient,
    type InterruptedMemoryJobDto,
    type MemoryQueryEmbeddingRetryCandidateDto,
    type ModelRouteDto,
    type NativeCaptureStatusDto,
    type OrchestrationVariableMapDto,
    type ProviderCatalogRollbackPlanDto,
    type ProviderConnectionDto,
    type ProviderProfileDto,
    type ProviderTemplateDto,
    type ReviewedPromptSendInput,
    type UpdateProviderConnectionInput,
    type UpsertCapabilityOverrideInput,
    type UpsertModelRouteInput,
} from '../lib/ipc/contracts';
import { t } from '../lib/i18n';
import { LorepiaClientError, normalizeClientError } from '../lib/ipc/errors';
import { ChatStreamController } from './controllers/chat-stream-controller';
import { ConversationController } from './controllers/conversation-controller';
import type { AppControllerContext } from './controllers/controller-context';
import { DiscoveryController } from './controllers/discovery-controller';
import { GenerationController } from './controllers/generation-controller';
import { ImportController } from './controllers/import-controller';
import { LibraryController } from './controllers/library-controller';
import { MemoryController } from './controllers/memory-controller';
import { ProviderController } from './controllers/provider-controller';
import { INITIAL_APP_STATE, type LorepiaAppState } from './app-state';
import { EpochGuard } from './operations/epoch-guard';

export {
    INITIAL_APP_STATE,
    type ChatState,
    type GreetingCatalogState,
    type ImportFlowState,
    type LorepiaAppState,
    type MemoryQueryRetryState,
    type SectionState,
} from './app-state';
export { discoveryCredentialTarget } from './provider-credential';

export interface RemoveMessageResult {
    mutationCommitted: boolean;
    messagesRefreshed: boolean;
    scopeKey: string | null;
}

const GENERATION_REATTACHMENT_UNAVAILABLE_MESSAGE = t('app.error.stream_lost');

function errorLabel(error: unknown): string {
    const normalized = normalizeClientError(error);
    const fallback: Record<string, string> = {
        'error.unexpected': t('error.unexpected'),
        'error.compatibility': t('error.compatibility'),
        'error.invalid_input': t('error.invalid_input'),
        'error.core_unavailable': t('error.core_unavailable'),
        'chat.generation_reattachment_unavailable': GENERATION_REATTACHMENT_UNAVAILABLE_MESSAGE,
        'provider.discovery.assistant_pricing_unavailable': t('app.error.remote_assistant_blocked'),
    };
    return fallback[normalized.messageKey] ?? normalized.messageKey;
}

function ensureCompatible(snapshot: BootstrapDto): void {
    if (
        snapshot.shell_api_version !== SUPPORTED_SHELL_API_VERSION ||
        snapshot.core_api_version !== SUPPORTED_CORE_API_VERSION ||
        snapshot.chat_event_version !== SUPPORTED_CHAT_EVENT_VERSION
    ) {
        throw new LorepiaClientError({
            code: 'incompatible_version',
            message_key: 'error.compatibility',
            recoverable: false,
            operation_id: null,
            field_errors: [],
        });
    }
}

function captureAnnouncement(status: NativeCaptureStatusDto, success: string): string {
    switch (status.clipboard_cleanup) {
        case 'cleared':
            return success;
        case 'already_replaced':
            return t('app.capture.clipboard_changed', { success });
        case 'clear_failed':
            return t('app.capture.clipboard_kept', { success });
    }
}

export class LorepiaAppController {
    private readonly mutable = writable<LorepiaAppState>(structuredClone(INITIAL_APP_STATE));
    readonly state: Readable<LorepiaAppState> = this.mutable;

    private readonly memoryController: MemoryController;
    private readonly providerController: ProviderController;
    private readonly discoveryController: DiscoveryController;
    private readonly streamController: ChatStreamController;
    private readonly generationController: GenerationController;
    private readonly libraryController: LibraryController;
    private readonly importController: ImportController;
    private readonly conversationController: ConversationController;

    private readonly appEpoch = new EpochGuard();

    constructor(private readonly client: LorepiaClient) {
        const context: AppControllerContext = {
            client,
            readState: () => get(this.mutable),
            update: (updater) => this.update(updater),
            announce: (message) => this.announce(message),
            errorLabel,
        };
        this.memoryController = new MemoryController(context, {
            isAppEpochCurrent: (epoch) => this.appEpoch.isCurrent(epoch),
        });
        this.providerController = new ProviderController(context, {
            captureAnnouncement,
            loadProviders: () => this.loadProviders(),
            loadProviderCapabilities: (modelRouteId) => this.loadProviderCapabilities(modelRouteId),
            refreshProviderModelSync: (jobId) => this.refreshProviderModelSync(jobId),
        });
        this.discoveryController = new DiscoveryController(context, {
            captureAnnouncement,
            updateProviderWorkspace: (updater) =>
                context.update((state) => ({
                    ...state,
                    providers: {
                        ...state.providers,
                        workspace: updater(state.providers.workspace),
                    },
                })),
            loadProviders: () => this.loadProviders(),
            refreshProviderDiscovery: (sessionId) => this.refreshProviderDiscovery(sessionId),
            pollSelectedProviderDiscoveryEvents: () => this.pollSelectedProviderDiscoveryEvents(),
        });
        this.streamController = new ChatStreamController(context, {
            refreshMemoryQueryRetries: () => this.refreshMemoryQueryRetries(),
        });
        this.generationController = new GenerationController(context, this.streamController, {
            clearMemoryQueryRetryNotice: () => this.clearMemoryQueryRetryNotice(),
            invalidateMemoryQueryRetries: () => this.memoryController.invalidateQueryRetries(),
            refreshMemoryQueryRetries: () => this.refreshMemoryQueryRetries(),
            activeBranchHead: (state) => this.activeBranchHead(state),
        });
        this.libraryController = new LibraryController(context, (epoch) =>
            this.appEpoch.isCurrent(epoch),
        );
        this.importController = new ImportController(context);
        this.conversationController = new ConversationController(context, {
            detachStream: () => this.streamController.detachStream(),
            invalidateMemoryQueryRetries: () => this.memoryController.invalidateQueryRetries(),
            refreshMemoryQueryRetries: () => this.refreshMemoryQueryRetries(),
            resumePendingGeneration: (messages) =>
                this.streamController.resumePendingGeneration(messages),
            selectBranch: (branchId) => this.selectBranch(branchId),
            activeBranchHead: (state) => this.activeBranchHead(state),
        });
    }

    beginNewGenerationOperation(): void {
        this.generationController.beginNewGenerationOperation();
    }

    stageGenerationAttemptRetry(generationAttemptId: string): boolean {
        return this.generationController.stageGenerationAttemptRetry(generationAttemptId);
    }

    private update(updater: (state: LorepiaAppState) => LorepiaAppState): void {
        this.mutable.update(updater);
    }

    private announce(message: string): void {
        this.update((state) => ({ ...state, announcement: message }));
    }

    async start(): Promise<void> {
        const epoch = this.appEpoch.advance();
        this.update((state) => ({
            ...state,
            bootstrap: { ...state.bootstrap, phase: 'loading', error: null },
        }));
        try {
            const snapshot = await this.client.bootstrapSnapshot();
            ensureCompatible(snapshot);
            if (!this.appEpoch.isCurrent(epoch)) return;
            this.update((state) => ({
                ...state,
                bootstrap: { phase: 'ready', error: null, value: snapshot },
            }));
            await Promise.all([
                this.loadLibrary(epoch),
                this.loadProviders(),
                this.connectMemorySupervisor(epoch),
            ]);
        } catch (error: unknown) {
            if (!this.appEpoch.isCurrent(epoch)) return;
            this.update((state) => ({
                ...state,
                bootstrap: { phase: 'error', error: errorLabel(error), value: null },
            }));
        }
    }

    private connectMemorySupervisor(parentEpoch: number): Promise<void> {
        return this.memoryController.connectMemorySupervisor(parentEpoch);
    }

    loadLibrary(parentEpoch = this.appEpoch.current()): Promise<void> {
        return this.libraryController.load(parentEpoch);
    }

    beginImport(): Promise<void> {
        return this.importController.begin();
    }

    commitImport(): Promise<void> {
        return this.importController.commit();
    }

    discardImport(): Promise<void> {
        return this.importController.discard();
    }

    selectCharacter(character: CharacterDto): Promise<void> {
        return this.conversationController.selectCharacter(character);
    }

    selectGreeting(greetingId: string): boolean {
        return this.conversationController.selectGreeting(greetingId);
    }

    openNewConversation(): Promise<boolean> {
        return this.conversationController.openNewConversation();
    }

    selectConversation(conversation: ConversationDto): Promise<boolean> {
        return this.conversationController.selectConversation(conversation);
    }

    selectBranch(branchId: string): Promise<void> {
        return this.conversationController.selectBranch(branchId);
    }

    createBranch(fromMessageId: string | null): Promise<void> {
        return this.conversationController.createBranch(fromMessageId);
    }

    setConversationMode(mode: ConversationMode): Promise<void> {
        return this.conversationController.setConversationMode(mode);
    }

    setRoomGenerationTarget(
        conversationId: string | null,
        branchId: string | null,
        target: GenerationTargetDto | null | undefined,
    ): void {
        this.generationController.setRoomGenerationTarget(conversationId, branchId, target);
    }

    runtimeGenerationSelection(): GenerationSelectionInput | null {
        return this.generationController.runtimeGenerationSelection();
    }

    sendMessage(
        content: string,
        variableOverrides: OrchestrationVariableMapDto = { values: [] },
    ): Promise<boolean> {
        return this.generationController.sendMessage(content, variableOverrides);
    }

    sendReviewedPrompt(input: ReviewedPromptSendInput): Promise<boolean> {
        return this.generationController.sendReviewedPrompt(input);
    }

    editUserMessage(messageId: string, replacementText: string): Promise<boolean> {
        return this.generationController.editUserMessage(messageId, replacementText);
    }

    regenerateAssistantMessage(messageId: string): Promise<boolean> {
        return this.generationController.regenerateAssistantMessage(messageId);
    }

    removeMessage(messageId: string): Promise<RemoveMessageResult> {
        return this.conversationController.removeMessage(messageId);
    }

    refreshMemoryQueryRetries(): Promise<void> {
        return this.memoryController.refreshMemoryQueryRetries();
    }

    clearMemoryQueryRetryNotice(): void {
        this.memoryController.clearMemoryQueryRetryNotice();
    }

    retryInterruptedMemoryJob(
        job: InterruptedMemoryJobDto,
        acknowledgeUnknownOutcome: boolean,
    ): Promise<boolean> {
        return this.memoryController.retryInterruptedMemoryJob(job, acknowledgeUnknownOutcome);
    }

    retryMemoryQueryEmbedding(
        candidate: MemoryQueryEmbeddingRetryCandidateDto,
        acknowledgeUnknownOutcome: boolean,
    ): Promise<boolean> {
        return this.memoryController.retryMemoryQueryEmbedding(
            candidate,
            acknowledgeUnknownOutcome,
        );
    }

    private activeBranchHead(state: LorepiaAppState): string | null {
        const activeBranchId = state.conversation_state?.active_branch_id;
        return state.branches.find((item) => item.id === activeBranchId)?.head_message_id ?? null;
    }

    cancelGeneration(): Promise<void> {
        return this.generationController.cancelGeneration();
    }

    loadProviders(): Promise<void> {
        return this.providerController.loadProviders();
    }

    captureProviderCredential(target: CredentialTargetDto): Promise<boolean> {
        return this.providerController.captureProviderCredential(target);
    }

    deleteProviderCredential(target: CredentialTargetDto): Promise<void> {
        return this.providerController.deleteProviderCredential(target);
    }

    createProviderConnection(input: CreateProviderConnectionInput): Promise<boolean> {
        return this.providerController.createProviderConnection(input);
    }

    updateProviderConnection(input: UpdateProviderConnectionInput): Promise<boolean> {
        return this.providerController.updateProviderConnection(input);
    }

    deleteProviderConnection(connectionId: string): Promise<boolean> {
        return this.providerController.deleteProviderConnection(connectionId);
    }

    upsertProviderModelRoute(input: UpsertModelRouteInput): Promise<boolean> {
        return this.providerController.upsertProviderModelRoute(input);
    }

    deleteProviderModelRoute(modelRouteId: string): Promise<boolean> {
        return this.providerController.deleteProviderModelRoute(modelRouteId);
    }

    upsertProviderGenerationPreset(input: GenerationPresetInput): Promise<boolean> {
        return this.providerController.upsertProviderGenerationPreset(input);
    }

    deleteProviderGenerationPreset(generationPresetId: string): Promise<boolean> {
        return this.providerController.deleteProviderGenerationPreset(generationPresetId);
    }

    validateProviderGenerationPresetCandidate(input: GenerationPresetInput): Promise<boolean> {
        return this.providerController.validateProviderGenerationPresetCandidate(input);
    }

    previewProviderRequestCandidate(input: GenerationPresetInput): Promise<void> {
        return this.providerController.previewProviderRequestCandidate(input);
    }

    previewSelectedProviderRequest(): Promise<boolean> {
        return this.providerController.previewSelectedProviderRequest();
    }

    loadProviderCapabilities(modelRouteId: string): Promise<void> {
        return this.providerController.loadProviderCapabilities(modelRouteId);
    }

    inspectEffectiveProviderCapability(key: CapabilityKeyInput): Promise<void> {
        return this.providerController.inspectEffectiveProviderCapability(key);
    }

    upsertProviderCapabilityOverride(input: UpsertCapabilityOverrideInput): Promise<boolean> {
        return this.providerController.upsertProviderCapabilityOverride(input);
    }

    deleteProviderCapabilityOverride(observationId: string): Promise<void> {
        return this.providerController.deleteProviderCapabilityOverride(observationId);
    }

    selectProviderGenerationTarget(
        modelRouteId: string | null,
        generationPresetId: string | null,
    ): Promise<boolean> {
        return this.providerController.selectProviderGenerationTarget(
            modelRouteId,
            generationPresetId,
        );
    }

    selectLegacyProviderProfile(profileId: string): Promise<boolean> {
        return this.providerController.selectLegacyProviderProfile(profileId);
    }

    setPreservePartialGenerations(preserve: boolean): Promise<boolean> {
        return this.providerController.setPreservePartialGenerations(preserve);
    }

    startProviderModelSync(connectionId: string): Promise<void> {
        return this.providerController.startProviderModelSync(connectionId);
    }

    refreshProviderModelSync(jobId: string): Promise<void> {
        return this.providerController.refreshProviderModelSync(jobId);
    }

    approveProviderModelSync(jobId: string): Promise<void> {
        return this.providerController.approveProviderModelSync(jobId);
    }

    cancelProviderModelSync(jobId: string): Promise<void> {
        return this.providerController.cancelProviderModelSync(jobId);
    }

    beginProviderDiscovery(
        request:
            | { kind: 'site'; input: BeginProviderDiscoveryInput }
            | { kind: 'curl'; input: BeginProviderDiscoveryCurlInput },
    ): Promise<boolean> {
        return this.discoveryController.beginProviderDiscovery(request);
    }

    refreshProviderDiscovery(sessionId: string): Promise<void> {
        return this.discoveryController.refreshProviderDiscovery(sessionId);
    }

    runProviderDiscoveryAssistant(): Promise<void> {
        return this.discoveryController.runProviderDiscoveryAssistant();
    }

    resumeProviderDiscoveryAssistantCoreHostAction(): Promise<void> {
        return this.discoveryController.resumeProviderDiscoveryAssistantCoreHostAction();
    }

    approveProviderDiscoveryAssistantRetry(): Promise<void> {
        return this.discoveryController.approveProviderDiscoveryAssistantRetry();
    }

    requestProviderDiscoveryAssistantRevision(): Promise<void> {
        return this.discoveryController.requestProviderDiscoveryAssistantRevision();
    }

    acceptProviderDiscoveryAssistantDraft(): Promise<void> {
        return this.discoveryController.acceptProviderDiscoveryAssistantDraft();
    }

    recordProviderDiscoveryAssistantFailure(
        kind: DiscoveryAssistantFailureKindInput,
        retryable: boolean,
    ): Promise<void> {
        return this.discoveryController.recordProviderDiscoveryAssistantFailure(kind, retryable);
    }

    interruptProviderDiscoveryAssistant(
        outcome: DiscoveryAssistantInterruptionOutcomeInput,
    ): Promise<void> {
        return this.discoveryController.interruptProviderDiscoveryAssistant(outcome);
    }

    restartProviderDiscoveryAssistantAfterInterruption(): Promise<void> {
        return this.discoveryController.restartProviderDiscoveryAssistantAfterInterruption();
    }

    pollSelectedProviderDiscoveryEvents(): Promise<void> {
        return this.discoveryController.pollSelectedProviderDiscoveryEvents();
    }

    continueProviderDiscovery(action: ContinueProviderDiscoveryActionInput): Promise<boolean> {
        return this.discoveryController.continueProviderDiscovery(action);
    }

    supplyProviderDiscoveryDocumentEvidence(documentUrl: string): Promise<boolean> {
        return this.discoveryController.supplyProviderDiscoveryDocumentEvidence(documentUrl);
    }

    supplyProviderDiscoveryCurlEvidence(): Promise<boolean> {
        return this.discoveryController.supplyProviderDiscoveryCurlEvidence();
    }

    cancelProviderDiscovery(): Promise<void> {
        return this.discoveryController.cancelProviderDiscovery();
    }

    commitProviderDiscovery(): Promise<boolean> {
        return this.discoveryController.commitProviderDiscovery();
    }

    recoverProviderDiscoveries(): Promise<void> {
        return this.discoveryController.recoverProviderDiscoveries();
    }

    continueProviderDiscoveryCompensation(resume: boolean): Promise<void> {
        return this.discoveryController.continueProviderDiscoveryCompensation(resume);
    }

    pickProviderCatalogImport(): Promise<void> {
        return this.providerController.pickProviderCatalogImport();
    }

    activateProviderCatalogImport(): Promise<void> {
        return this.providerController.activateProviderCatalogImport();
    }

    discardProviderCatalogImport(): Promise<void> {
        return this.providerController.discardProviderCatalogImport();
    }

    diffProviderCatalogRevisions(fromRevision: number, toRevision: number): Promise<void> {
        return this.providerController.diffProviderCatalogRevisions(fromRevision, toRevision);
    }

    prepareProviderCatalogRollback(targetRevision: number): Promise<void> {
        return this.providerController.prepareProviderCatalogRollback(targetRevision);
    }

    activateProviderCatalogRollback(plan?: ProviderCatalogRollbackPlanDto): Promise<void> {
        return this.providerController.activateProviderCatalogRollback(plan);
    }

    destroy(): void {
        this.appEpoch.advance();
        this.conversationController.destroy();
        this.memoryController.invalidateQueryRetries();
        this.providerController.destroy();
        this.discoveryController.destroy();
        this.memoryController.disconnectMemorySupervisor();
        this.streamController.detachStream();
    }
}

export type {
    GenerationPresetDto,
    ModelRouteDto,
    ProviderConnectionDto,
    ProviderProfileDto,
    ProviderTemplateDto,
};
