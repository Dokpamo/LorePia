import { get, writable, type Readable } from 'svelte/store';

import {
    SUPPORTED_CHAT_EVENT_VERSION,
    SUPPORTED_CORE_API_VERSION,
    SUPPORTED_SHELL_API_VERSION,
    type AppSettingsDto,
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
    type MemoryJobRetryReceiptDto,
    type MemoryQueryEmbeddingRetryCandidateDto,
    type MemorySupervisorStatusDto,
    type ModelRouteDto,
    type ModelSyncJobDto,
    type NativeCaptureStatusDto,
    type OrchestrationVariableMapDto,
    type ProviderCatalogRollbackPlanDto,
    type ProviderConnectionDto,
    type ProviderDiscoverySessionDto,
    type ProviderProfileDto,
    type ProviderTemplateDto,
    type ProviderWorkspaceDto,
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
import { GenerationController } from './controllers/generation-controller';
import { ImportController } from './controllers/import-controller';
import { LibraryController } from './controllers/library-controller';
import { INITIAL_APP_STATE, type LorepiaAppState } from './app-state';
import { EpochGuard } from './operations/epoch-guard';
import { SerializedMutation } from './operations/serialized-mutation';
import { credentialKey, discoveryCredentialTarget } from './provider-credential';
import {
    drainProviderDiscoveryEvents,
    loadProviderDiscoverySnapshot,
    mergeProviderDiscoverySnapshot,
    storeProviderDiscoverySession,
} from './provider-discovery-flow';

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

const MAX_MEMORY_QUERY_RETRY_CANDIDATES = 16;
const MAX_INTERRUPTED_MEMORY_JOBS = 16;
// The DTO unions are claims about untrusted IPC payloads, not guarantees,
// so these guards compare over `string` instead of the narrowed literal types.
const MEMORY_JOB_RETRY_KINDS: readonly string[] = ['summary', 'embedding'];
const MEMORY_JOB_RETRY_STATUSES: readonly string[] = ['queued'];

function isRetryableMemoryQueryCandidate(
    candidate: MemoryQueryEmbeddingRetryCandidateDto,
    conversationId: string,
    branchId: string,
): boolean {
    const retryableStatus =
        candidate.status === 'interrupted' ||
        candidate.status === 'failed' ||
        candidate.status === 'cancelled';
    return (
        typeof candidate.id === 'string' &&
        candidate.id.length > 0 &&
        retryableStatus &&
        Number.isSafeInteger(candidate.revision) &&
        candidate.revision >= 0 &&
        candidate.revision < Number.MAX_SAFE_INTEGER &&
        candidate.conversation_id === conversationId &&
        candidate.branch_id === branchId &&
        (candidate.error_code === null || typeof candidate.error_code === 'string') &&
        candidate.requires_unknown_outcome_acknowledgement === (candidate.status === 'interrupted')
    );
}

function isInterruptedMemoryJob(
    job: InterruptedMemoryJobDto,
    conversationId: string,
    branchId: string,
): boolean {
    return (
        typeof job.memory_job_id === 'string' &&
        job.memory_job_id.length > 0 &&
        MEMORY_JOB_RETRY_KINDS.includes(job.kind) &&
        Number.isSafeInteger(job.revision) &&
        job.revision >= 0 &&
        job.revision < Number.MAX_SAFE_INTEGER &&
        job.conversation_id === conversationId &&
        job.branch_id === branchId &&
        typeof job.source_start_message_id === 'string' &&
        typeof job.source_end_message_id === 'string' &&
        Number.isSafeInteger(job.attempt) &&
        job.attempt >= 0 &&
        Number.isSafeInteger(job.interruption_count) &&
        job.interruption_count >= 0 &&
        (job.last_interrupted_at === null || typeof job.last_interrupted_at === 'string') &&
        (job.last_error_code === null || typeof job.last_error_code === 'string')
    );
}

function isQueuedMemoryJobRetryReceipt(
    receipt: MemoryJobRetryReceiptDto,
    job: InterruptedMemoryJobDto,
): boolean {
    return (
        receipt.memory_job_id === job.memory_job_id &&
        MEMORY_JOB_RETRY_STATUSES.includes(receipt.status) &&
        receipt.kind === job.kind &&
        receipt.revision === job.revision + 1 &&
        receipt.conversation_id === job.conversation_id &&
        receipt.branch_id === job.branch_id &&
        receipt.source_start_message_id === job.source_start_message_id &&
        receipt.source_end_message_id === job.source_end_message_id
    );
}

function isQueuedMemoryQueryRetryReceipt(
    receipt: MemoryQueryEmbeddingRetryCandidateDto,
    candidate: MemoryQueryEmbeddingRetryCandidateDto,
): boolean {
    return (
        receipt.id === candidate.id &&
        receipt.status === 'queued' &&
        receipt.revision === candidate.revision + 1 &&
        receipt.conversation_id === candidate.conversation_id &&
        receipt.branch_id === candidate.branch_id &&
        receipt.error_code === null &&
        !receipt.requires_unknown_outcome_acknowledgement
    );
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

function isMemorySupervisorStatus(value: unknown): value is MemorySupervisorStatusDto {
    if (typeof value !== 'object' || value === null) return false;
    const candidate = value as Record<string, unknown>;
    const allowedKeys = new Set([
        'sequence',
        'phase',
        'recovered_interrupted_jobs',
        'completed_jobs',
    ]);
    return (
        Object.keys(candidate).every((key) => allowedKeys.has(key)) &&
        Number.isSafeInteger(candidate.sequence) &&
        Number(candidate.sequence) >= 0 &&
        typeof candidate.phase === 'string' &&
        ['not_started', 'recovered', 'running', 'failed'].includes(candidate.phase) &&
        Number.isSafeInteger(candidate.recovered_interrupted_jobs) &&
        Number(candidate.recovered_interrupted_jobs) >= 0 &&
        Number.isSafeInteger(candidate.completed_jobs) &&
        Number(candidate.completed_jobs) >= 0
    );
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

    private readonly streamController: ChatStreamController;
    private readonly generationController: GenerationController;
    private readonly libraryController: LibraryController;
    private readonly importController: ImportController;
    private readonly conversationController: ConversationController;

    private readonly appEpoch = new EpochGuard();
    private readonly memoryQueryRetryEpoch = new EpochGuard();
    private readonly providerEpoch = new EpochGuard();
    private readonly providerSettingsEpoch = new EpochGuard();
    private readonly discoveryRequestEpoch = new EpochGuard();
    private readonly providerSettingsMutations = new SerializedMutation();
    private memorySupervisorUnlisten: (() => void) | null = null;

    constructor(private readonly client: LorepiaClient) {
        const context: AppControllerContext = {
            client,
            readState: () => get(this.mutable),
            update: (updater) => this.update(updater),
            announce: (message) => this.announce(message),
            errorLabel,
        };
        this.streamController = new ChatStreamController(context, {
            refreshMemoryQueryRetries: () => this.refreshMemoryQueryRetries(),
        });
        this.generationController = new GenerationController(context, this.streamController, {
            clearMemoryQueryRetryNotice: () => this.clearMemoryQueryRetryNotice(),
            invalidateMemoryQueryRetries: () => {
                this.memoryQueryRetryEpoch.advance();
            },
            refreshMemoryQueryRetries: () => this.refreshMemoryQueryRetries(),
            activeBranchHead: (state) => this.activeBranchHead(state),
        });
        this.libraryController = new LibraryController(context, (epoch) =>
            this.appEpoch.isCurrent(epoch),
        );
        this.importController = new ImportController(context);
        this.conversationController = new ConversationController(context, {
            detachStream: () => this.streamController.detachStream(),
            invalidateMemoryQueryRetries: () => {
                this.memoryQueryRetryEpoch.advance();
            },
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

    private async connectMemorySupervisor(parentEpoch: number): Promise<void> {
        this.memorySupervisorUnlisten?.();
        this.memorySupervisorUnlisten = null;
        this.update((state) => ({
            ...state,
            memory_supervisor: {
                ...state.memory_supervisor,
                phase: 'loading',
                error: null,
            },
        }));

        let subscriptionFailed = false;
        try {
            const unlisten = await this.client.subscribeMemorySupervisorStatus((status) => {
                if (!this.appEpoch.isCurrent(parentEpoch) || !isMemorySupervisorStatus(status))
                    return;
                this.applyMemorySupervisorStatus(status);
            });
            if (!this.appEpoch.isCurrent(parentEpoch)) {
                unlisten();
                return;
            }
            this.memorySupervisorUnlisten = unlisten;
        } catch {
            subscriptionFailed = true;
        }

        try {
            const status = await this.client.getMemorySupervisorStatus();
            if (!this.appEpoch.isCurrent(parentEpoch)) return;
            if (!isMemorySupervisorStatus(status)) {
                throw new Error('invalid memory supervisor status');
            }
            this.applyMemorySupervisorStatus(status);
        } catch {
            if (!this.appEpoch.isCurrent(parentEpoch)) return;
            this.update((state) => ({
                ...state,
                memory_supervisor: {
                    ...state.memory_supervisor,
                    phase: state.memory_supervisor.status === null ? 'error' : 'ready',
                    error:
                        state.memory_supervisor.status === null
                            ? t('memory_supervisor.error.status')
                            : null,
                },
            }));
            return;
        }

        if (subscriptionFailed) {
            this.update((state) => ({
                ...state,
                memory_supervisor: {
                    ...state.memory_supervisor,
                    error: t('memory_supervisor.error.subscribe'),
                },
            }));
        }
    }

    private applyMemorySupervisorStatus(status: MemorySupervisorStatusDto): void {
        this.update((state) => {
            const current = state.memory_supervisor.status;
            if (current !== null && status.sequence < current.sequence) return state;
            return {
                ...state,
                memory_supervisor: {
                    phase: 'ready',
                    error: null,
                    status,
                },
            };
        });
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

    async refreshMemoryQueryRetries(): Promise<void> {
        const state = get(this.mutable);
        const conversationId = state.selected_conversation?.id;
        const branchId = state.conversation_state?.active_branch_id;
        if (conversationId === undefined || branchId === undefined) {
            this.memoryQueryRetryEpoch.advance();
            this.update((current) => ({
                ...current,
                memory_query_retries: {
                    phase: 'idle',
                    error: null,
                    candidates: [],
                    interrupted_jobs: [],
                    busy_id: null,
                    notice: null,
                },
            }));
            return;
        }
        if (state.memory_query_retries.busy_id !== null) return;
        const requestEpoch = this.memoryQueryRetryEpoch.advance();
        this.update((current) => ({
            ...current,
            memory_query_retries: {
                ...current.memory_query_retries,
                phase: 'loading',
                error: null,
            },
        }));
        try {
            // Settled independently: the interrupted-job listing is a
            // supplementary surface, so its failure must never blank or fault
            // the query-embedding candidates the user came here to retry.
            const [candidateResult, jobResult] = await Promise.allSettled([
                this.client.listRetryableMemoryQueryEmbeddings({
                    conversation_id: conversationId,
                    branch_id: branchId,
                    limit: MAX_MEMORY_QUERY_RETRY_CANDIDATES,
                }),
                this.client.listInterruptedMemoryJobs({
                    conversation_id: conversationId,
                    branch_id: branchId,
                    limit: MAX_INTERRUPTED_MEMORY_JOBS,
                }),
            ]);
            if (candidateResult.status === 'rejected') throw candidateResult.reason;
            const candidates = candidateResult.value;
            const interruptedJobs = jobResult.status === 'fulfilled' ? jobResult.value : [];
            const jobListError =
                jobResult.status === 'rejected' ? errorLabel(jobResult.reason) : null;
            const current = get(this.mutable);
            if (
                !this.memoryQueryRetryEpoch.isCurrent(requestEpoch) ||
                current.selected_conversation?.id !== conversationId ||
                current.conversation_state?.active_branch_id !== branchId
            ) {
                return;
            }
            const uniqueIds = new Set(candidates.map((candidate) => candidate.id));
            const uniqueJobIds = new Set(interruptedJobs.map((job) => job.memory_job_id));
            if (
                candidates.length > MAX_MEMORY_QUERY_RETRY_CANDIDATES ||
                uniqueIds.size !== candidates.length ||
                !candidates.every((candidate) =>
                    isRetryableMemoryQueryCandidate(candidate, conversationId, branchId),
                ) ||
                interruptedJobs.length > MAX_INTERRUPTED_MEMORY_JOBS ||
                uniqueJobIds.size !== interruptedJobs.length ||
                !interruptedJobs.every((job) =>
                    isInterruptedMemoryJob(job, conversationId, branchId),
                )
            ) {
                this.update((value) => ({
                    ...value,
                    memory_query_retries: {
                        ...value.memory_query_retries,
                        phase: 'error',
                        error: t('memory.retry.error.list'),
                        busy_id: null,
                    },
                }));
                return;
            }
            this.update((value) => ({
                ...value,
                memory_query_retries: {
                    phase: 'ready',
                    error: jobListError,
                    candidates,
                    interrupted_jobs: interruptedJobs,
                    busy_id: null,
                    notice: value.memory_query_retries.notice,
                },
            }));
        } catch (error: unknown) {
            const current = get(this.mutable);
            if (
                !this.memoryQueryRetryEpoch.isCurrent(requestEpoch) ||
                current.selected_conversation?.id !== conversationId ||
                current.conversation_state?.active_branch_id !== branchId
            ) {
                return;
            }
            this.update((value) => ({
                ...value,
                memory_query_retries: {
                    ...value.memory_query_retries,
                    phase: 'error',
                    error: errorLabel(error),
                    busy_id: null,
                },
            }));
        }
    }

    clearMemoryQueryRetryNotice(): void {
        this.update((state) =>
            state.memory_query_retries.notice === null
                ? state
                : {
                      ...state,
                      memory_query_retries: {
                          ...state.memory_query_retries,
                          notice: null,
                      },
                  },
        );
    }

    async retryInterruptedMemoryJob(
        job: InterruptedMemoryJobDto,
        acknowledgeUnknownOutcome: boolean,
    ): Promise<boolean> {
        const state = get(this.mutable);
        const listedJob = state.memory_query_retries.interrupted_jobs.find(
            (value) => value.memory_job_id === job.memory_job_id,
        );
        if (listedJob?.revision !== job.revision) {
            this.announce(t('memory.retry.notice.reload'));
            return false;
        }
        if (
            listedJob.kind !== job.kind ||
            state.selected_conversation?.id !== listedJob.conversation_id ||
            state.conversation_state?.active_branch_id !== listedJob.branch_id ||
            !isInterruptedMemoryJob(listedJob, listedJob.conversation_id, listedJob.branch_id)
        ) {
            this.announce(t('memory.retry.notice.reload'));
            return false;
        }
        if (state.memory_query_retries.busy_id !== null) {
            this.announce(t('memory.retry.notice.busy_job'));
            return false;
        }
        if (!acknowledgeUnknownOutcome) {
            this.announce(t('memory.retry.notice.acknowledge'));
            return false;
        }
        this.memoryQueryRetryEpoch.advance();
        this.update((current) => ({
            ...current,
            memory_query_retries: {
                ...current.memory_query_retries,
                phase: 'loading',
                error: null,
                busy_id: listedJob.memory_job_id,
                notice: null,
            },
        }));
        try {
            const receipt = await this.client.retryInterruptedMemoryJob({
                conversation_id: listedJob.conversation_id,
                branch_id: listedJob.branch_id,
                memory_job_id: listedJob.memory_job_id,
                expected_revision: listedJob.revision,
                acknowledge_unknown_outcome: true,
            });
            const current = get(this.mutable);
            const sameRoom =
                current.selected_conversation?.id === listedJob.conversation_id &&
                current.conversation_state?.active_branch_id === listedJob.branch_id;
            if (!isQueuedMemoryJobRetryReceipt(receipt, listedJob)) {
                if (sameRoom) {
                    this.update((value) => ({
                        ...value,
                        memory_query_retries: {
                            ...value.memory_query_retries,
                            phase: 'error',
                            error: t('memory.retry.error.receipt'),
                            busy_id: null,
                        },
                    }));
                }
                return false;
            }
            if (!sameRoom) return true;
            const notice = t('memory.retry.notice.job_requeued');
            this.update((value) => ({
                ...value,
                memory_query_retries: {
                    ...value.memory_query_retries,
                    phase: 'ready',
                    error: null,
                    interrupted_jobs: value.memory_query_retries.interrupted_jobs.filter(
                        (listed) =>
                            listed.memory_job_id !== listedJob.memory_job_id ||
                            listed.revision !== listedJob.revision,
                    ),
                    busy_id: null,
                    notice,
                },
            }));
            this.announce(notice);
            return true;
        } catch (error: unknown) {
            const current = get(this.mutable);
            if (
                current.selected_conversation?.id === listedJob.conversation_id &&
                current.conversation_state?.active_branch_id === listedJob.branch_id
            ) {
                this.update((value) => ({
                    ...value,
                    memory_query_retries: {
                        ...value.memory_query_retries,
                        phase: 'error',
                        error: errorLabel(error),
                        busy_id: null,
                    },
                }));
            }
            return false;
        }
    }

    async retryMemoryQueryEmbedding(
        candidate: MemoryQueryEmbeddingRetryCandidateDto,
        acknowledgeUnknownOutcome: boolean,
    ): Promise<boolean> {
        const state = get(this.mutable);
        const listedCandidate = state.memory_query_retries.candidates.find(
            (value) => value.id === candidate.id,
        );
        if (listedCandidate?.revision !== candidate.revision) {
            this.announce(t('memory.retry.notice.reload'));
            return false;
        }
        if (
            listedCandidate.status !== candidate.status ||
            listedCandidate.requires_unknown_outcome_acknowledgement !==
                candidate.requires_unknown_outcome_acknowledgement ||
            state.selected_conversation?.id !== listedCandidate.conversation_id ||
            state.conversation_state?.active_branch_id !== listedCandidate.branch_id ||
            !isRetryableMemoryQueryCandidate(
                listedCandidate,
                listedCandidate.conversation_id,
                listedCandidate.branch_id,
            )
        ) {
            this.announce(t('memory.retry.notice.reload'));
            return false;
        }
        if (state.memory_query_retries.busy_id !== null) {
            this.announce(t('memory.retry.notice.busy_query'));
            return false;
        }
        if (listedCandidate.status === 'interrupted' && !acknowledgeUnknownOutcome) {
            this.announce(t('memory.retry.notice.acknowledge'));
            return false;
        }
        this.memoryQueryRetryEpoch.advance();
        this.update((current) => ({
            ...current,
            memory_query_retries: {
                ...current.memory_query_retries,
                phase: 'loading',
                error: null,
                busy_id: listedCandidate.id,
                notice: null,
            },
        }));
        try {
            const receipt = await this.client.retryMemoryQueryEmbedding({
                conversation_id: listedCandidate.conversation_id,
                branch_id: listedCandidate.branch_id,
                id: listedCandidate.id,
                expected_revision: listedCandidate.revision,
                acknowledge_unknown_outcome:
                    listedCandidate.status === 'interrupted' && acknowledgeUnknownOutcome,
            });
            if (!isQueuedMemoryQueryRetryReceipt(receipt, listedCandidate)) {
                const current = get(this.mutable);
                if (
                    current.selected_conversation?.id === listedCandidate.conversation_id &&
                    current.conversation_state?.active_branch_id === listedCandidate.branch_id
                ) {
                    this.update((value) => ({
                        ...value,
                        memory_query_retries: {
                            ...value.memory_query_retries,
                            phase: 'error',
                            error: t('memory.retry.error.receipt'),
                            busy_id: null,
                        },
                    }));
                }
                return false;
            }
            const current = get(this.mutable);
            if (
                current.selected_conversation?.id !== listedCandidate.conversation_id ||
                current.conversation_state?.active_branch_id !== listedCandidate.branch_id
            ) {
                return true;
            }
            const notice = t('memory.retry.notice.query_requeued');
            this.update((value) => ({
                ...value,
                memory_query_retries: {
                    phase: 'ready',
                    error: null,
                    candidates: value.memory_query_retries.candidates.filter(
                        (listed) =>
                            listed.id !== listedCandidate.id ||
                            listed.revision !== listedCandidate.revision,
                    ),
                    interrupted_jobs: value.memory_query_retries.interrupted_jobs,
                    busy_id: null,
                    notice,
                },
            }));
            this.announce(notice);
            return true;
        } catch (error: unknown) {
            const current = get(this.mutable);
            if (
                current.selected_conversation?.id === listedCandidate.conversation_id &&
                current.conversation_state?.active_branch_id === listedCandidate.branch_id
            ) {
                this.update((value) => ({
                    ...value,
                    memory_query_retries: {
                        ...value.memory_query_retries,
                        phase: 'error',
                        error: errorLabel(error),
                        busy_id: null,
                    },
                }));
            }
            return false;
        }
    }

    private activeBranchHead(state: LorepiaAppState): string | null {
        const activeBranchId = state.conversation_state?.active_branch_id;
        return state.branches.find((item) => item.id === activeBranchId)?.head_message_id ?? null;
    }

    cancelGeneration(): Promise<void> {
        return this.generationController.cancelGeneration();
    }

    async loadProviders(): Promise<void> {
        const epoch = this.providerEpoch.advance();
        const settingsEpoch = this.providerSettingsEpoch.current();
        this.update((state) => ({
            ...state,
            providers: { ...state.providers, phase: 'loading', error: null },
        }));
        try {
            const [overview, discoveries, catalogStatus, catalogHistory] = await Promise.all([
                this.client.getProviderOverview(),
                this.client.listProviderDiscoveries(50),
                this.client.providerCatalogStatus(),
                this.client.providerCatalogHistory(50, null, null),
            ]);
            const routeGroups = await Promise.all(
                overview.connections.map((connection) =>
                    this.client.listModelRoutes(connection.id),
                ),
            );
            const routes = routeGroups.flat();
            const presetGroups = await Promise.all(
                routes.map((route) => this.client.listGenerationPresets(route.id)),
            );
            const retainedLegacyProfileIds = new Set(
                overview.legacy_profiles.map((profile) => profile.id),
            );
            const credentialTargets: CredentialTargetDto[] = [
                ...overview.connections
                    .filter(
                        (connection) =>
                            connection.credential_binding_required &&
                            !retainedLegacyProfileIds.has(connection.id),
                    )
                    .map((connection): CredentialTargetDto => ({
                        kind: 'connection',
                        connection_id: connection.id,
                    })),
                ...overview.legacy_profiles.map((profile): CredentialTargetDto => ({
                    kind: 'legacy_profile',
                    provider_profile_id: profile.id,
                })),
                ...discoveries.flatMap((session): CredentialTargetDto[] => {
                    const target = discoveryCredentialTarget(session);
                    return target === null ? [] : [target];
                }),
            ];
            const credentialStates = await Promise.all(
                credentialTargets.map(async (target) => ({
                    target,
                    status: (await this.client.credentialStatus(target)).status,
                })),
            );
            const modelSyncGroups = await Promise.all(
                overview.connections.map((connection) =>
                    this.client.listProviderModelSyncs(connection.id, 20),
                ),
            );
            if (!this.providerEpoch.isCurrent(epoch)) return;
            this.update((state) => ({
                ...state,
                providers: {
                    phase: 'ready',
                    error: null,
                    workspace: {
                        templates: overview.templates,
                        connections: overview.connections,
                        legacy_profiles: overview.legacy_profiles,
                        routes,
                        presets: presetGroups.flat(),
                        settings: this.providerSettingsEpoch.isCurrent(settingsEpoch)
                            ? overview.settings
                            : state.providers.workspace.settings,
                        credential_statuses: Object.fromEntries(
                            credentialStates.map(({ target, status }) => [
                                credentialKey(target),
                                status,
                            ]),
                        ),
                        request_preview: state.providers.workspace.request_preview,
                        selected_capability_model_route_id:
                            state.providers.workspace.selected_capability_model_route_id,
                        capability_observations: state.providers.workspace.capability_observations,
                        capability_parameter_specs:
                            state.providers.workspace.capability_parameter_specs,
                        effective_capability: state.providers.workspace.effective_capability,
                        model_sync_jobs: modelSyncGroups
                            .flat()
                            .sort((left, right) => right.updated_at.localeCompare(left.updated_at)),
                        selected_model_sync_job_id:
                            state.providers.workspace.selected_model_sync_job_id,
                        model_sync_event: state.providers.workspace.model_sync_event,
                        discoveries,
                        selected_discovery_id: state.providers.workspace.selected_discovery_id,
                        discovery_candidates: state.providers.workspace.discovery_candidates,
                        discovery_evidence: state.providers.workspace.discovery_evidence,
                        discovery_approvals: state.providers.workspace.discovery_approvals,
                        discovery_review: state.providers.workspace.discovery_review,
                        discovery_approval_proposal:
                            state.providers.workspace.discovery_approval_proposal,
                        discovery_review_proposal:
                            state.providers.workspace.discovery_review_proposal,
                        discovery_assistant_resume_boundary:
                            state.providers.workspace.discovery_assistant_resume_boundary,
                        discovery_assistant_host_action:
                            state.providers.workspace.discovery_assistant_host_action,
                        discovery_event: state.providers.workspace.discovery_event,
                        discovery_compensation_steps:
                            state.providers.workspace.discovery_compensation_steps,
                        discovery_recovery_results:
                            state.providers.workspace.discovery_recovery_results,
                        catalog_status: catalogStatus,
                        catalog_history: catalogHistory,
                        pending_catalog_import: state.providers.workspace.pending_catalog_import,
                        pending_catalog_rollback:
                            state.providers.workspace.pending_catalog_rollback,
                        catalog_diff: state.providers.workspace.catalog_diff,
                    },
                },
            }));
        } catch (error: unknown) {
            if (!this.providerEpoch.isCurrent(epoch)) return;
            this.update((state) => ({
                ...state,
                providers: {
                    ...state.providers,
                    phase: 'error',
                    error: errorLabel(error),
                },
            }));
        }
    }

    async captureProviderCredential(target: CredentialTargetDto): Promise<boolean> {
        if (this.isRetainedLegacyConnectionCredentialTarget(target)) return false;
        try {
            const capture = await this.client.captureCredential(target);
            const status = await this.client.credentialStatus(target);
            this.update((state) => ({
                ...state,
                providers: {
                    ...state.providers,
                    workspace: {
                        ...state.providers.workspace,
                        credential_statuses: {
                            ...state.providers.workspace.credential_statuses,
                            [credentialKey(target)]: status.status,
                        },
                    },
                },
            }));
            this.announce(captureAnnouncement(capture, t('provider.notice.credential_stored')));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async deleteProviderCredential(target: CredentialTargetDto): Promise<void> {
        if (this.isRetainedLegacyConnectionCredentialTarget(target)) return;
        try {
            await this.client.deleteCredential(target);
            this.update((state) => ({
                ...state,
                providers: {
                    ...state.providers,
                    workspace: {
                        ...state.providers.workspace,
                        credential_statuses: {
                            ...state.providers.workspace.credential_statuses,
                            [credentialKey(target)]: 'missing',
                        },
                    },
                },
            }));
            this.announce(t('provider.notice.credential_deleted'));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    private isRetainedLegacyConnectionCredentialTarget(target: CredentialTargetDto): boolean {
        return (
            target.kind === 'connection' &&
            get(this.mutable).providers.workspace.legacy_profiles.some(
                (profile) => profile.id === target.connection_id,
            )
        );
    }

    async createProviderConnection(input: CreateProviderConnectionInput): Promise<boolean> {
        try {
            await this.client.createProviderConnection(input);
            await this.loadProviders();
            this.announce(t('provider.notice.connection_created'));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async updateProviderConnection(input: UpdateProviderConnectionInput): Promise<boolean> {
        try {
            await this.client.upsertProviderConnection(input);
            await this.loadProviders();
            this.announce(t('provider.notice.connection_updated'));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async deleteProviderConnection(connectionId: string): Promise<boolean> {
        try {
            await this.client.deleteProviderConnection(connectionId);
            await this.loadProviders();
            this.announce(t('provider.notice.connection_deleted'));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async upsertProviderModelRoute(input: UpsertModelRouteInput): Promise<boolean> {
        try {
            await this.client.upsertModelRoute(input);
            await this.loadProviders();
            this.announce(t('provider.notice.route_saved'));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async deleteProviderModelRoute(modelRouteId: string): Promise<boolean> {
        try {
            await this.client.deleteModelRoute(modelRouteId);
            await this.loadProviders();
            this.announce(t('provider.notice.route_deleted'));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async upsertProviderGenerationPreset(input: GenerationPresetInput): Promise<boolean> {
        try {
            await this.client.upsertGenerationPreset(input);
            await this.loadProviders();
            this.announce(t('provider.notice.preset_saved'));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async deleteProviderGenerationPreset(generationPresetId: string): Promise<boolean> {
        try {
            await this.client.deleteGenerationPreset(generationPresetId);
            await this.loadProviders();
            this.announce(t('provider.notice.preset_deleted'));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async validateProviderGenerationPresetCandidate(
        input: GenerationPresetInput,
    ): Promise<boolean> {
        try {
            await this.client.validateGenerationPresetCandidate(input);
            this.announce(t('provider.notice.preset_valid'));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async previewProviderRequestCandidate(input: GenerationPresetInput): Promise<void> {
        try {
            const preview = await this.client.previewProviderRequestCandidate(input);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                request_preview: preview,
            }));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async previewSelectedProviderRequest(): Promise<boolean> {
        const settings = get(this.mutable).providers.workspace.settings;
        if (
            settings.selected_model_route_id === null ||
            settings.selected_generation_preset_id === null
        ) {
            this.announce(t('provider.notice.no_default_route'));
            return false;
        }
        try {
            const preview = await this.client.previewProviderRequest({
                model_route_id: settings.selected_model_route_id,
                generation_preset_id: settings.selected_generation_preset_id,
            });
            this.update((state) => ({
                ...state,
                providers: {
                    ...state.providers,
                    workspace: { ...state.providers.workspace, request_preview: preview },
                },
            }));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    private updateProviderWorkspace(
        updater: (workspace: ProviderWorkspaceDto) => ProviderWorkspaceDto,
    ): void {
        this.update((state) => ({
            ...state,
            providers: {
                ...state.providers,
                workspace: updater(state.providers.workspace),
            },
        }));
    }

    private storeProviderSettings(settings: AppSettingsDto): void {
        this.providerSettingsEpoch.advance();
        this.updateProviderWorkspace((workspace) => ({ ...workspace, settings }));
    }

    private enqueueProviderSettingsMutation<T>(mutation: () => Promise<T>): Promise<T> {
        return this.providerSettingsMutations.enqueue(mutation);
    }

    private storeModelSyncJob(job: ModelSyncJobDto): void {
        this.updateProviderWorkspace((workspace) => ({
            ...workspace,
            model_sync_jobs: [
                job,
                ...workspace.model_sync_jobs.filter((candidate) => candidate.id !== job.id),
            ],
            selected_model_sync_job_id: job.id,
        }));
    }

    private storeDiscoverySession(session: ProviderDiscoverySessionDto): void {
        this.updateProviderWorkspace((workspace) =>
            storeProviderDiscoverySession(workspace, session),
        );
    }

    async loadProviderCapabilities(modelRouteId: string): Promise<void> {
        if (modelRouteId === '') return;
        try {
            const [observations, parameterSpecs] = await Promise.all([
                this.client.listCapabilityObservations(modelRouteId),
                this.client.effectiveParameterSpecs(modelRouteId),
            ]);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                selected_capability_model_route_id: modelRouteId,
                capability_observations: observations,
                capability_parameter_specs: parameterSpecs,
                effective_capability: null,
            }));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async inspectEffectiveProviderCapability(key: CapabilityKeyInput): Promise<void> {
        const routeId = get(this.mutable).providers.workspace.selected_capability_model_route_id;
        if (routeId === null) return;
        try {
            const capability = await this.client.effectiveCapability(routeId, key);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                effective_capability: capability,
            }));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async upsertProviderCapabilityOverride(input: UpsertCapabilityOverrideInput): Promise<boolean> {
        try {
            await this.client.upsertUserCapabilityOverride(input);
            await this.loadProviderCapabilities(input.model_route_id);
            this.announce(t('provider.notice.override_saved'));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async deleteProviderCapabilityOverride(observationId: string): Promise<void> {
        const routeId = get(this.mutable).providers.workspace.selected_capability_model_route_id;
        if (routeId === null) return;
        try {
            await this.client.deleteUserCapabilityOverride(routeId, observationId);
            await this.loadProviderCapabilities(routeId);
            this.announce(t('provider.notice.override_deleted'));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async selectProviderGenerationTarget(
        modelRouteId: string | null,
        generationPresetId: string | null,
    ): Promise<boolean> {
        if ((modelRouteId === null) !== (generationPresetId === null)) return false;
        return this.enqueueProviderSettingsMutation(async () => {
            try {
                const settings = await this.client.selectGenerationTarget(
                    modelRouteId === null || generationPresetId === null
                        ? null
                        : {
                              model_route_id: modelRouteId,
                              generation_preset_id: generationPresetId,
                          },
                );
                this.storeProviderSettings(settings);
                this.announce(
                    modelRouteId === null
                        ? t('provider.notice.target_cleared')
                        : t('provider.notice.target_saved'),
                );
                return true;
            } catch (error: unknown) {
                this.announce(errorLabel(error));
                return false;
            }
        });
    }

    async selectLegacyProviderProfile(profileId: string): Promise<boolean> {
        return this.enqueueProviderSettingsMutation(async () => {
            const workspace = get(this.mutable).providers.workspace;
            if (!workspace.legacy_profiles.some((profile) => profile.id === profileId))
                return false;
            try {
                const settings = await this.client.updateSettings({
                    ...workspace.settings,
                    selected_provider_profile_id: profileId,
                    selected_model_route_id: null,
                    selected_generation_preset_id: null,
                });
                this.storeProviderSettings(settings);
                this.announce(t('provider.notice.existing_target_saved'));
                return true;
            } catch (error: unknown) {
                this.announce(errorLabel(error));
                return false;
            }
        });
    }

    async setPreservePartialGenerations(preserve: boolean): Promise<boolean> {
        return this.enqueueProviderSettingsMutation(async () => {
            const current = get(this.mutable).providers.workspace.settings;
            try {
                const settings = await this.client.updateSettings({
                    ...current,
                    preserve_partial_generations: preserve,
                });
                this.storeProviderSettings(settings);
                this.announce(t('provider.notice.partial_saved'));
                return true;
            } catch (error: unknown) {
                this.announce(errorLabel(error));
                return false;
            }
        });
    }

    async startProviderModelSync(connectionId: string): Promise<void> {
        try {
            const started = await this.client.startProviderModelSync(connectionId);
            await this.refreshProviderModelSync(started.job_id);
            this.announce(t('provider.notice.sync_started'));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async refreshProviderModelSync(jobId: string): Promise<void> {
        try {
            const [job, events] = await Promise.all([
                this.client.getProviderModelSync(jobId),
                this.client.pollProviderModelSyncEvents(jobId, 100),
            ]);
            const latestEvent = events.at(-1) ?? null;
            for (const event of events) {
                await this.client.ackProviderModelSyncEvent(jobId, event.sequence);
            }
            this.storeModelSyncJob(job);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                model_sync_event: latestEvent ?? workspace.model_sync_event,
            }));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async approveProviderModelSync(jobId: string): Promise<void> {
        const job = get(this.mutable).providers.workspace.model_sync_jobs.find(
            (candidate) => candidate.id === jobId,
        );
        if (job?.review === null || job?.review === undefined) return;
        try {
            this.storeModelSyncJob(
                await this.client.approveProviderModelSync(jobId, job.review.sha256),
            );
            await this.loadProviders();
            this.announce(t('provider.notice.sync_applied'));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async cancelProviderModelSync(jobId: string): Promise<void> {
        try {
            this.storeModelSyncJob(await this.client.cancelProviderModelSync(jobId));
            this.announce(t('provider.notice.sync_cancelled'));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async beginProviderDiscovery(
        request:
            | { kind: 'site'; input: BeginProviderDiscoveryInput }
            | { kind: 'curl'; input: BeginProviderDiscoveryCurlInput },
    ): Promise<boolean> {
        try {
            let session: ProviderDiscoverySessionDto;
            let capture: NativeCaptureStatusDto | null = null;
            if (request.kind === 'site') {
                session = await this.client.beginProviderDiscovery(request.input);
            } else {
                const captured = await this.client.beginProviderDiscoveryCurl(request.input);
                session = captured.session;
                capture = captured.capture;
            }
            this.storeDiscoverySession(session);
            await this.refreshProviderDiscovery(session.id);
            await this.pollSelectedProviderDiscoveryEvents();
            this.announce(
                capture === null
                    ? t('provider.notice.discovery_started')
                    : captureAnnouncement(capture, t('provider.notice.discovery_started_curl')),
            );
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async refreshProviderDiscovery(sessionId: string): Promise<void> {
        const requestEpoch = this.discoveryRequestEpoch.advance();
        this.updateProviderWorkspace((workspace) => ({
            ...workspace,
            selected_discovery_id: sessionId,
        }));
        await this.refreshProviderDiscoveryAtEpoch(sessionId, requestEpoch);
    }

    private isCurrentDiscoveryRequest(sessionId: string, requestEpoch: number): boolean {
        return (
            this.discoveryRequestEpoch.isCurrent(requestEpoch) &&
            get(this.mutable).providers.workspace.selected_discovery_id === sessionId
        );
    }

    private async refreshProviderDiscoveryAtEpoch(
        sessionId: string,
        requestEpoch: number,
    ): Promise<void> {
        try {
            const snapshot = await loadProviderDiscoverySnapshot(this.client, sessionId, () =>
                this.isCurrentDiscoveryRequest(sessionId, requestEpoch),
            );
            if (snapshot === null || !this.isCurrentDiscoveryRequest(sessionId, requestEpoch))
                return;
            this.updateProviderWorkspace((workspace) =>
                mergeProviderDiscoverySnapshot(workspace, snapshot),
            );
        } catch (error: unknown) {
            if (!this.isCurrentDiscoveryRequest(sessionId, requestEpoch)) return;
            this.announce(errorLabel(error));
        }
    }

    private selectedProviderDiscoveryId(): string | null {
        return get(this.mutable).providers.workspace.selected_discovery_id;
    }

    async runProviderDiscoveryAssistant(): Promise<void> {
        const sessionId = this.selectedProviderDiscoveryId();
        if (sessionId === null) return;
        try {
            const hostAction = await this.client.runProviderDiscoveryAssistantTurn(sessionId);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                discovery_assistant_host_action: hostAction,
            }));
            await this.refreshProviderDiscovery(sessionId);
            this.announce(t('provider.notice.assistant_ready'));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async resumeProviderDiscoveryAssistantCoreHostAction(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.resumeProviderDiscoveryAssistantCoreHostAction(sessionId),
        );
    }

    async approveProviderDiscoveryAssistantRetry(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.approveProviderDiscoveryAssistantRetry(sessionId),
        );
    }

    async requestProviderDiscoveryAssistantRevision(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.requestProviderDiscoveryAssistantRevision(sessionId),
        );
    }

    async acceptProviderDiscoveryAssistantDraft(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.acceptProviderDiscoveryAssistantDraft(sessionId),
        );
    }

    async recordProviderDiscoveryAssistantFailure(
        kind: DiscoveryAssistantFailureKindInput,
        retryable: boolean,
    ): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.recordProviderDiscoveryAssistantFailure(sessionId, kind, retryable),
        );
    }

    async interruptProviderDiscoveryAssistant(
        outcome: DiscoveryAssistantInterruptionOutcomeInput,
    ): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.interruptProviderDiscoveryAssistant(sessionId, outcome),
        );
    }

    async restartProviderDiscoveryAssistantAfterInterruption(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.restartProviderDiscoveryAssistantAfterInterruption(sessionId),
        );
    }

    private async mutateSelectedDiscoveryAssistant(
        action: (sessionId: string) => Promise<ProviderDiscoverySessionDto>,
    ): Promise<void> {
        const sessionId = this.selectedProviderDiscoveryId();
        if (sessionId === null) return;
        try {
            this.storeDiscoverySession(await action(sessionId));
            await this.refreshProviderDiscovery(sessionId);
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async pollSelectedProviderDiscoveryEvents(): Promise<void> {
        const selectedId = get(this.mutable).providers.workspace.selected_discovery_id;
        if (selectedId === null) return;
        const requestEpoch = this.discoveryRequestEpoch.advance();
        try {
            const result = await drainProviderDiscoveryEvents(this.client, selectedId, () =>
                this.isCurrentDiscoveryRequest(selectedId, requestEpoch),
            );
            if (result === null || !this.isCurrentDiscoveryRequest(selectedId, requestEpoch))
                return;
            if (result.latest !== null) {
                this.updateProviderWorkspace((workspace) => ({
                    ...workspace,
                    discovery_event: result.latest,
                }));
            }
            if (!this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) return;
            await this.refreshProviderDiscoveryAtEpoch(selectedId, requestEpoch);
            if (!result.drained && this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) {
                this.announce(t('provider.notice.events_truncated'));
            }
        } catch (error: unknown) {
            if (!this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) return;
            this.announce(errorLabel(error));
        }
    }

    async continueProviderDiscovery(
        action: ContinueProviderDiscoveryActionInput,
    ): Promise<boolean> {
        const workspace = get(this.mutable).providers.workspace;
        const session = workspace.discoveries.find(
            (candidate) => candidate.id === workspace.selected_discovery_id,
        );
        if (!session?.action_required) {
            this.announce(t('provider.notice.no_next_step'));
            return false;
        }
        const actionId = globalThis.crypto.randomUUID();
        try {
            const next = await this.client.continueProviderDiscovery({
                session_id: session.id,
                action_id: actionId,
                expected_revision: session.revision,
                action,
            });
            this.storeDiscoverySession(next);
            await this.refreshProviderDiscovery(next.id);
            await this.pollSelectedProviderDiscoveryEvents();
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async supplyProviderDiscoveryDocumentEvidence(documentUrl: string): Promise<boolean> {
        const workspace = get(this.mutable).providers.workspace;
        const session = workspace.discoveries.find(
            (candidate) => candidate.id === workspace.selected_discovery_id,
        );
        if (session === undefined || documentUrl.trim() === '') return false;
        try {
            this.storeDiscoverySession(
                await this.client.supplyProviderDiscoveryDocumentEvidence(
                    session.id,
                    session.revision,
                    documentUrl.trim(),
                ),
            );
            await this.refreshProviderDiscovery(session.id);
            await this.pollSelectedProviderDiscoveryEvents();
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async supplyProviderDiscoveryCurlEvidence(): Promise<boolean> {
        const workspace = get(this.mutable).providers.workspace;
        const session = workspace.discoveries.find(
            (candidate) => candidate.id === workspace.selected_discovery_id,
        );
        if (session === undefined) return false;
        try {
            const captured = await this.client.supplyProviderDiscoveryCurlEvidence(
                session.id,
                session.revision,
            );
            this.storeDiscoverySession(captured.session);
            await this.refreshProviderDiscovery(session.id);
            await this.pollSelectedProviderDiscoveryEvents();
            this.announce(captureAnnouncement(captured.capture, t('provider.notice.curl_added')));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async cancelProviderDiscovery(): Promise<void> {
        const workspace = get(this.mutable).providers.workspace;
        const session = workspace.discoveries.find(
            (candidate) => candidate.id === workspace.selected_discovery_id,
        );
        if (session === undefined) return;
        try {
            this.storeDiscoverySession(
                await this.client.cancelProviderDiscovery(session.id, session.revision),
            );
            await this.refreshProviderDiscovery(session.id);
            this.announce(t('provider.notice.discovery_cancelled'));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async commitProviderDiscovery(): Promise<boolean> {
        const sessionId = get(this.mutable).providers.workspace.selected_discovery_id;
        if (sessionId === null) return false;
        try {
            await this.client.commitProviderDiscovery(sessionId);
            await this.loadProviders();
            this.announce(t('provider.notice.connection_saved'));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async recoverProviderDiscoveries(): Promise<void> {
        try {
            const results = await this.client.recoverProviderDiscovery();
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                discovery_recovery_results: results,
            }));
            await this.loadProviders();
            this.announce(
                results.length === 0
                    ? t('provider.notice.no_recovery')
                    : t('provider.notice.recovered', { count: results.length }),
            );
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async continueProviderDiscoveryCompensation(resume: boolean): Promise<void> {
        const sessionId = get(this.mutable).providers.workspace.selected_discovery_id;
        if (sessionId === null) return;
        try {
            const session = resume
                ? await this.client.resumeProviderDiscoveryCompensation(sessionId)
                : await this.client.continueProviderDiscoveryCompensation(sessionId);
            this.storeDiscoverySession(session);
            await this.refreshProviderDiscovery(sessionId);
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async pickProviderCatalogImport(): Promise<void> {
        try {
            const ticket = await this.client.pickProviderCatalogImport();
            if (ticket === null) return;
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                pending_catalog_import: ticket,
            }));
            this.announce(t('provider.notice.catalog_plan'));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async activateProviderCatalogImport(): Promise<void> {
        const ticket = get(this.mutable).providers.workspace.pending_catalog_import;
        if (ticket === null) return;
        try {
            const result = await this.client.activateProviderCatalogImport(ticket.ticket_id);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                catalog_status: result.status,
                pending_catalog_import: null,
                catalog_diff: result.diff,
            }));
            await this.loadProviders();
            this.announce(t('provider.notice.catalog_applied'));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async discardProviderCatalogImport(): Promise<void> {
        const ticket = get(this.mutable).providers.workspace.pending_catalog_import;
        if (ticket === null) return;
        try {
            await this.client.discardProviderCatalogImport(ticket.ticket_id);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                pending_catalog_import: null,
            }));
            this.announce(t('provider.notice.catalog_discarded'));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async diffProviderCatalogRevisions(fromRevision: number, toRevision: number): Promise<void> {
        try {
            const catalogDiff = await this.client.diffProviderCatalogRevisions(
                fromRevision,
                toRevision,
            );
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                catalog_diff: catalogDiff,
            }));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async prepareProviderCatalogRollback(targetRevision: number): Promise<void> {
        try {
            const plan = await this.client.prepareProviderCatalogRollback(targetRevision);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                pending_catalog_rollback: plan,
                catalog_diff: plan.catalog_plan.diff,
            }));
            this.announce(t('provider.notice.rollback_plan'));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async activateProviderCatalogRollback(plan?: ProviderCatalogRollbackPlanDto): Promise<void> {
        const exactPlan = plan ?? get(this.mutable).providers.workspace.pending_catalog_rollback;
        if (exactPlan === null) return;
        try {
            const result = await this.client.activateProviderCatalogRollback(exactPlan);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                catalog_status: result.status,
                pending_catalog_rollback: null,
                catalog_diff: exactPlan.catalog_plan.diff,
            }));
            await this.loadProviders();
            this.announce(t('provider.notice.rolled_back'));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    destroy(): void {
        this.appEpoch.advance();
        this.conversationController.destroy();
        this.memoryQueryRetryEpoch.advance();
        this.providerEpoch.advance();
        this.discoveryRequestEpoch.advance();
        this.memorySupervisorUnlisten?.();
        this.memorySupervisorUnlisten = null;
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
