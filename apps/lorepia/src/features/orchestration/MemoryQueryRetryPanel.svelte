<script lang="ts">
    import { t, tr } from '../../lib/i18n';
    import type { LorepiaAppController, MemoryQueryRetryState } from '../../app/app-controller';
    import type {
        InterruptedMemoryJobDto,
        MemoryQueryEmbeddingRetryCandidateDto,
    } from '../../lib/ipc/contracts';

    interface Props {
        state: MemoryQueryRetryState;
        controller: LorepiaAppController;
        headingId: string;
    }

    let { state: retryState, controller, headingId }: Props = $props();
    let acknowledgedCandidateKey = $state<string | null>(null);
    let acknowledgedJobKey = $state<string | null>(null);

    const visible = $derived(
        retryState.phase === 'loading' ||
            retryState.error != null ||
            retryState.notice != null ||
            retryState.candidates.length > 0 ||
            retryState.interrupted_jobs.length > 0,
    );

    function candidateKey(candidate: MemoryQueryEmbeddingRetryCandidateDto): string {
        return `${candidate.id}:${String(candidate.revision)}`;
    }

    function statusLabel(candidate: MemoryQueryEmbeddingRetryCandidateDto): string {
        if (candidate.status === 'interrupted') return t('memory.retry.status.interrupted');
        if (candidate.status === 'failed') return t('memory.retry.status.failed');
        if (candidate.status === 'cancelled') return t('memory.retry.status.cancelled');
        return t('memory.retry.status.queued');
    }

    function jobKey(job: InterruptedMemoryJobDto): string {
        return `${job.memory_job_id}:${String(job.revision)}`;
    }

    function jobKindLabel(job: InterruptedMemoryJobDto): string {
        return job.kind === 'summary'
            ? t('memory.retry.job.summary')
            : t('memory.retry.job.embedding');
    }

    async function retryJob(job: InterruptedMemoryJobDto): Promise<void> {
        const accepted = await controller.retryInterruptedMemoryJob(job, true);
        if (accepted) acknowledgedJobKey = null;
    }

    async function retry(
        candidate: MemoryQueryEmbeddingRetryCandidateDto,
        acknowledgeUnknownOutcome: boolean,
    ): Promise<void> {
        const accepted = await controller.retryMemoryQueryEmbedding(
            candidate,
            acknowledgeUnknownOutcome,
        );
        if (accepted) acknowledgedCandidateKey = null;
    }

    $effect(() => {
        const acknowledged = acknowledgedCandidateKey;
        if (acknowledged === null) return;
        const stillPresent = retryState.candidates.some(
            (candidate) =>
                candidate.status === 'interrupted' && candidateKey(candidate) === acknowledged,
        );
        if (!stillPresent) acknowledgedCandidateKey = null;
    });

    $effect(() => {
        const acknowledged = acknowledgedJobKey;
        if (acknowledged === null) return;
        const stillPresent = retryState.interrupted_jobs.some(
            (job) => jobKey(job) === acknowledged,
        );
        if (!stillPresent) acknowledgedJobKey = null;
    });
</script>

{#if visible}
    <section class="memory-query-retry" aria-labelledby={headingId}>
        <header>
            <div>
                <h3 id={headingId}>{$tr('memory.retry.heading')}</h3>
                <p>
                    {$tr('memory.retry.hint')}
                </p>
            </div>
            <button
                type="button"
                disabled={retryState.phase === 'loading' || retryState.busy_id !== null}
                onclick={() => void controller.refreshMemoryQueryRetries()}
            >
                {$tr('common.refresh')}
            </button>
        </header>

        {#if retryState.notice != null}
            <p class="retry-notice" role="status">{retryState.notice}</p>
        {/if}
        {#if retryState.error != null}
            <p class="retry-error" role="alert">{retryState.error}</p>
        {:else if retryState.phase === 'loading' && retryState.candidates.length === 0 && retryState.interrupted_jobs.length === 0}
            <p class="retry-loading" role="status">{$tr('memory.retry.loading')}</p>
        {/if}

        {#if retryState.candidates.length > 0}
            <ul>
                {#each retryState.candidates as candidate, index (candidate.id)}
                    {@const key = candidateKey(candidate)}
                    {@const warningId = `${headingId}-unknown-${String(index)}`}
                    <li>
                        <div class="retry-summary">
                            <strong>{statusLabel(candidate)}</strong>
                            {#if candidate.error_code !== null}
                                <span
                                    >{$tr('memory.retry.error_code', {
                                        code: candidate.error_code.slice(0, 256),
                                    })}</span
                                >
                            {/if}
                        </div>

                        {#if candidate.status === 'interrupted'}
                            {#if acknowledgedCandidateKey === key}
                                <p class="retry-warning" id={warningId}>
                                    {$tr('memory.retry.ack.embedding')}
                                </p>
                                <button
                                    class="danger"
                                    type="button"
                                    aria-describedby={warningId}
                                    disabled={retryState.busy_id !== null}
                                    onclick={() => void retry(candidate, true)}
                                >
                                    {$tr('memory.retry.confirm')}
                                </button>
                            {:else}
                                <button
                                    class="danger"
                                    type="button"
                                    disabled={retryState.busy_id !== null}
                                    onclick={() => {
                                        acknowledgedCandidateKey = key;
                                    }}
                                >
                                    {$tr('memory.retry.review')}
                                </button>
                            {/if}
                        {:else}
                            <button
                                class="primary"
                                type="button"
                                disabled={retryState.busy_id !== null}
                                onclick={() => void retry(candidate, false)}
                            >
                                {$tr('memory.retry.start')}
                            </button>
                        {/if}
                    </li>
                {/each}
            </ul>
        {/if}

        {#if retryState.interrupted_jobs.length > 0}
            <h4>{$tr('memory.retry.jobs.heading')}</h4>
            <ul>
                {#each retryState.interrupted_jobs as job, index (job.memory_job_id)}
                    {@const key = jobKey(job)}
                    {@const jobWarningId = `${headingId}-job-unknown-${String(index)}`}
                    <li>
                        <div class="retry-summary">
                            <strong>{jobKindLabel(job)}</strong>
                            <span>
                                {$tr('memory.retry.job.attempts', {
                                    attempt: job.attempt,
                                    interruptions: job.interruption_count,
                                })}
                            </span>
                            {#if job.last_error_code !== null}
                                <span
                                    >{$tr('memory.retry.error_code', {
                                        code: job.last_error_code.slice(0, 256),
                                    })}</span
                                >
                            {/if}
                        </div>

                        {#if acknowledgedJobKey === key}
                            <p class="retry-warning" id={jobWarningId}>
                                {$tr('memory.retry.ack.job')}
                            </p>
                            <button
                                class="danger"
                                type="button"
                                aria-describedby={jobWarningId}
                                disabled={retryState.busy_id !== null}
                                onclick={() => void retryJob(job)}
                            >
                                {$tr('memory.retry.confirm.job')}
                            </button>
                        {:else}
                            <button
                                class="danger"
                                type="button"
                                disabled={retryState.busy_id !== null}
                                onclick={() => {
                                    acknowledgedJobKey = key;
                                }}
                            >
                                {$tr('memory.retry.review.job')}
                            </button>
                        {/if}
                    </li>
                {/each}
            </ul>
        {/if}
    </section>
{/if}

<style>
    .memory-query-retry {
        display: grid;
        gap: 10px;
        padding: 12px;
        border: 1px solid var(--status-warning-border);
        border-radius: 14px;
        background: var(--status-warning-bg);
    }

    header,
    li {
        display: flex;
        gap: 10px;
        align-items: center;
        justify-content: space-between;
    }

    h3,
    h4,
    p {
        margin: 0;
    }

    h4 {
        color: var(--ink-muted);
        font-size: 0.8rem;
    }

    header p,
    .retry-summary span,
    .retry-loading {
        color: var(--ink-muted);
        font-size: 0.75rem;
    }

    ul {
        display: grid;
        gap: 8px;
        margin: 0;
        padding: 0;
        list-style: none;
    }

    li {
        flex-wrap: wrap;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--surface);
    }

    .retry-summary {
        display: grid;
        gap: 2px;
    }

    .retry-warning,
    .retry-error {
        width: 100%;
        font-size: 0.78rem;
    }

    .retry-warning {
        color: var(--status-warning-fg);
    }

    .retry-error {
        padding: 8px 10px;
        border: 1px solid var(--status-error-border);
        border-radius: var(--radius-sm);
        color: var(--status-error-fg);
        background: var(--status-error-bg);
    }

    .retry-notice {
        color: var(--status-success-fg);
        font-size: 0.78rem;
    }

    @media (max-width: 720px) {
        header,
        li {
            align-items: stretch;
            flex-direction: column;
        }

        header > button,
        li > button {
            width: 100%;
        }
    }
</style>
