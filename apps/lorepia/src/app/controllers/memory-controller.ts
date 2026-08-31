import type {
    InterruptedMemoryJobDto,
    MemoryJobRetryReceiptDto,
    MemoryQueryEmbeddingRetryCandidateDto,
    MemorySupervisorStatusDto,
} from '../../lib/ipc/contracts';
import { t } from '../../lib/i18n';
import { EpochGuard } from '../operations/epoch-guard';
import type { AppControllerContext } from './controller-context';

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

interface MemoryControllerHooks {
    isAppEpochCurrent(epoch: number): boolean;
}

export class MemoryController {
    private readonly memoryQueryRetryEpoch = new EpochGuard();
    private memorySupervisorUnlisten: (() => void) | null = null;

    constructor(
        private readonly context: AppControllerContext,
        private readonly hooks: MemoryControllerHooks,
    ) {}

    async connectMemorySupervisor(parentEpoch: number): Promise<void> {
        this.memorySupervisorUnlisten?.();
        this.memorySupervisorUnlisten = null;
        this.context.update((state) => ({
            ...state,
            memory_supervisor: {
                ...state.memory_supervisor,
                phase: 'loading',
                error: null,
            },
        }));

        let subscriptionFailed = false;
        try {
            const unlisten = await this.context.client.subscribeMemorySupervisorStatus((status) => {
                if (!this.hooks.isAppEpochCurrent(parentEpoch) || !isMemorySupervisorStatus(status))
                    return;
                this.applyMemorySupervisorStatus(status);
            });
            if (!this.hooks.isAppEpochCurrent(parentEpoch)) {
                unlisten();
                return;
            }
            this.memorySupervisorUnlisten = unlisten;
        } catch {
            subscriptionFailed = true;
        }

        try {
            const status = await this.context.client.getMemorySupervisorStatus();
            if (!this.hooks.isAppEpochCurrent(parentEpoch)) return;
            if (!isMemorySupervisorStatus(status)) {
                throw new Error('invalid memory supervisor status');
            }
            this.applyMemorySupervisorStatus(status);
        } catch {
            if (!this.hooks.isAppEpochCurrent(parentEpoch)) return;
            this.context.update((state) => ({
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
            this.context.update((state) => ({
                ...state,
                memory_supervisor: {
                    ...state.memory_supervisor,
                    error: t('memory_supervisor.error.subscribe'),
                },
            }));
        }
    }

    private applyMemorySupervisorStatus(status: MemorySupervisorStatusDto): void {
        this.context.update((state) => {
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

    async refreshMemoryQueryRetries(): Promise<void> {
        const state = this.context.readState();
        const conversationId = state.selected_conversation?.id;
        const branchId = state.conversation_state?.active_branch_id;
        if (conversationId === undefined || branchId === undefined) {
            this.memoryQueryRetryEpoch.advance();
            this.context.update((current) => ({
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
        this.context.update((current) => ({
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
                this.context.client.listRetryableMemoryQueryEmbeddings({
                    conversation_id: conversationId,
                    branch_id: branchId,
                    limit: MAX_MEMORY_QUERY_RETRY_CANDIDATES,
                }),
                this.context.client.listInterruptedMemoryJobs({
                    conversation_id: conversationId,
                    branch_id: branchId,
                    limit: MAX_INTERRUPTED_MEMORY_JOBS,
                }),
            ]);
            if (candidateResult.status === 'rejected') throw candidateResult.reason;
            const candidates = candidateResult.value;
            const interruptedJobs = jobResult.status === 'fulfilled' ? jobResult.value : [];
            const jobListError =
                jobResult.status === 'rejected' ? this.context.errorLabel(jobResult.reason) : null;
            const current = this.context.readState();
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
                this.context.update((value) => ({
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
            this.context.update((value) => ({
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
            const current = this.context.readState();
            if (
                !this.memoryQueryRetryEpoch.isCurrent(requestEpoch) ||
                current.selected_conversation?.id !== conversationId ||
                current.conversation_state?.active_branch_id !== branchId
            ) {
                return;
            }
            this.context.update((value) => ({
                ...value,
                memory_query_retries: {
                    ...value.memory_query_retries,
                    phase: 'error',
                    error: this.context.errorLabel(error),
                    busy_id: null,
                },
            }));
        }
    }

    clearMemoryQueryRetryNotice(): void {
        this.context.update((state) =>
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
        const state = this.context.readState();
        const listedJob = state.memory_query_retries.interrupted_jobs.find(
            (value) => value.memory_job_id === job.memory_job_id,
        );
        if (listedJob?.revision !== job.revision) {
            this.context.announce(t('memory.retry.notice.reload'));
            return false;
        }
        if (
            listedJob.kind !== job.kind ||
            state.selected_conversation?.id !== listedJob.conversation_id ||
            state.conversation_state?.active_branch_id !== listedJob.branch_id ||
            !isInterruptedMemoryJob(listedJob, listedJob.conversation_id, listedJob.branch_id)
        ) {
            this.context.announce(t('memory.retry.notice.reload'));
            return false;
        }
        if (state.memory_query_retries.busy_id !== null) {
            this.context.announce(t('memory.retry.notice.busy_job'));
            return false;
        }
        if (!acknowledgeUnknownOutcome) {
            this.context.announce(t('memory.retry.notice.acknowledge'));
            return false;
        }
        this.memoryQueryRetryEpoch.advance();
        this.context.update((current) => ({
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
            const receipt = await this.context.client.retryInterruptedMemoryJob({
                conversation_id: listedJob.conversation_id,
                branch_id: listedJob.branch_id,
                memory_job_id: listedJob.memory_job_id,
                expected_revision: listedJob.revision,
                acknowledge_unknown_outcome: true,
            });
            const current = this.context.readState();
            const sameRoom =
                current.selected_conversation?.id === listedJob.conversation_id &&
                current.conversation_state?.active_branch_id === listedJob.branch_id;
            if (!isQueuedMemoryJobRetryReceipt(receipt, listedJob)) {
                if (sameRoom) {
                    this.context.update((value) => ({
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
            this.context.update((value) => ({
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
            this.context.announce(notice);
            return true;
        } catch (error: unknown) {
            const current = this.context.readState();
            if (
                current.selected_conversation?.id === listedJob.conversation_id &&
                current.conversation_state?.active_branch_id === listedJob.branch_id
            ) {
                this.context.update((value) => ({
                    ...value,
                    memory_query_retries: {
                        ...value.memory_query_retries,
                        phase: 'error',
                        error: this.context.errorLabel(error),
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
        const state = this.context.readState();
        const listedCandidate = state.memory_query_retries.candidates.find(
            (value) => value.id === candidate.id,
        );
        if (listedCandidate?.revision !== candidate.revision) {
            this.context.announce(t('memory.retry.notice.reload'));
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
            this.context.announce(t('memory.retry.notice.reload'));
            return false;
        }
        if (state.memory_query_retries.busy_id !== null) {
            this.context.announce(t('memory.retry.notice.busy_query'));
            return false;
        }
        if (listedCandidate.status === 'interrupted' && !acknowledgeUnknownOutcome) {
            this.context.announce(t('memory.retry.notice.acknowledge'));
            return false;
        }
        this.memoryQueryRetryEpoch.advance();
        this.context.update((current) => ({
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
            const receipt = await this.context.client.retryMemoryQueryEmbedding({
                conversation_id: listedCandidate.conversation_id,
                branch_id: listedCandidate.branch_id,
                id: listedCandidate.id,
                expected_revision: listedCandidate.revision,
                acknowledge_unknown_outcome:
                    listedCandidate.status === 'interrupted' && acknowledgeUnknownOutcome,
            });
            if (!isQueuedMemoryQueryRetryReceipt(receipt, listedCandidate)) {
                const current = this.context.readState();
                if (
                    current.selected_conversation?.id === listedCandidate.conversation_id &&
                    current.conversation_state?.active_branch_id === listedCandidate.branch_id
                ) {
                    this.context.update((value) => ({
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
            const current = this.context.readState();
            if (
                current.selected_conversation?.id !== listedCandidate.conversation_id ||
                current.conversation_state?.active_branch_id !== listedCandidate.branch_id
            ) {
                return true;
            }
            const notice = t('memory.retry.notice.query_requeued');
            this.context.update((value) => ({
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
            this.context.announce(notice);
            return true;
        } catch (error: unknown) {
            const current = this.context.readState();
            if (
                current.selected_conversation?.id === listedCandidate.conversation_id &&
                current.conversation_state?.active_branch_id === listedCandidate.branch_id
            ) {
                this.context.update((value) => ({
                    ...value,
                    memory_query_retries: {
                        ...value.memory_query_retries,
                        phase: 'error',
                        error: this.context.errorLabel(error),
                        busy_id: null,
                    },
                }));
            }
            return false;
        }
    }

    invalidateQueryRetries(): void {
        this.memoryQueryRetryEpoch.advance();
    }

    disconnectMemorySupervisor(): void {
        this.memorySupervisorUnlisten?.();
        this.memorySupervisorUnlisten = null;
    }
}
