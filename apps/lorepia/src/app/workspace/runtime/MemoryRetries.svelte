<script lang="ts">
    import './runtime.css';
    import { t, tr } from '../../../lib/i18n';
    import type { LorepiaAppController, MemoryQueryRetryState } from '../../app-controller';
    import type {
        InterruptedMemoryJobDto,
        MemoryQueryEmbeddingRetryCandidateDto,
    } from '../../../lib/ipc/contracts';

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
    <section class="ui-settings-group" aria-labelledby={headingId}>
        <header>
            <div>
                <h3 class="ui-settings-heading" id={headingId}>{$tr('memory.retry.heading')}</h3>
                <p>
                    {$tr('memory.retry.hint')}
                </p>
            </div>
            <button
                class="ui-submit ui-pressable"
                type="button"
                disabled={retryState.phase === 'loading' || retryState.busy_id !== null}
                onclick={() => void controller.refreshMemoryQueryRetries()}
                ><span class="ui-press-visual">
                    {$tr('common.refresh')}
                </span></button
            >
        </header>

        {#if retryState.notice != null}
            <p role="status">{retryState.notice}</p>
        {/if}
        {#if retryState.error != null}
            <p role="alert">{retryState.error}</p>
        {:else if retryState.phase === 'loading' && retryState.candidates.length === 0 && retryState.interrupted_jobs.length === 0}
            <p role="status">{$tr('memory.retry.loading')}</p>
        {/if}

        {#if retryState.candidates.length > 0}
            <ul class="ui-runtime-list">
                {#each retryState.candidates as candidate, index (candidate.id)}
                    {@const key = candidateKey(candidate)}
                    {@const warningId = `${headingId}-unknown-${String(index)}`}
                    <li>
                        <div>
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
                                <p id={warningId}>
                                    {$tr('memory.retry.ack.embedding')}
                                </p>
                                <button
                                    class="ui-submit ui-pressable"
                                    type="button"
                                    aria-describedby={warningId}
                                    disabled={retryState.busy_id !== null}
                                    onclick={() => void retry(candidate, true)}
                                    ><span class="ui-press-visual">
                                        {$tr('memory.retry.confirm')}
                                    </span></button
                                >
                            {:else}
                                <button
                                    class="ui-submit ui-pressable"
                                    type="button"
                                    disabled={retryState.busy_id !== null}
                                    onclick={() => {
                                        acknowledgedCandidateKey = key;
                                    }}
                                    ><span class="ui-press-visual">
                                        {$tr('memory.retry.review')}
                                    </span></button
                                >
                            {/if}
                        {:else}
                            <button
                                class="ui-submit ui-pressable"
                                type="button"
                                disabled={retryState.busy_id !== null}
                                onclick={() => void retry(candidate, false)}
                                ><span class="ui-press-visual">
                                    {$tr('memory.retry.start')}
                                </span></button
                            >
                        {/if}
                    </li>
                {/each}
            </ul>
        {/if}

        {#if retryState.interrupted_jobs.length > 0}
            <h4 class="ui-settings-heading">{$tr('memory.retry.jobs.heading')}</h4>
            <ul class="ui-runtime-list">
                {#each retryState.interrupted_jobs as job, index (job.memory_job_id)}
                    {@const key = jobKey(job)}
                    {@const jobWarningId = `${headingId}-job-unknown-${String(index)}`}
                    <li>
                        <div>
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
                            <p id={jobWarningId}>
                                {$tr('memory.retry.ack.job')}
                            </p>
                            <button
                                class="ui-submit ui-pressable"
                                type="button"
                                aria-describedby={jobWarningId}
                                disabled={retryState.busy_id !== null}
                                onclick={() => void retryJob(job)}
                                ><span class="ui-press-visual">
                                    {$tr('memory.retry.confirm.job')}
                                </span></button
                            >
                        {:else}
                            <button
                                class="ui-submit ui-pressable"
                                type="button"
                                disabled={retryState.busy_id !== null}
                                onclick={() => {
                                    acknowledgedJobKey = key;
                                }}
                                ><span class="ui-press-visual">
                                    {$tr('memory.retry.review.job')}
                                </span></button
                            >
                        {/if}
                    </li>
                {/each}
            </ul>
        {/if}
    </section>
{/if}
