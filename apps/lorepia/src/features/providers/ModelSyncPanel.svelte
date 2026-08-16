<script lang="ts">
    import { tr } from '../../lib/i18n';
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
    }

    let { appState, controller }: Props = $props();
    let selectedConnectionId = $state('');
    let busy = $state(false);

    const workspace = $derived(appState.providers.workspace);
    const selectedJob = $derived(
        workspace.model_sync_jobs.find((job) => job.id === workspace.selected_model_sync_job_id) ??
            null,
    );
    const selectedEvent = $derived(
        workspace.model_sync_event?.job_id === selectedJob?.id ? workspace.model_sync_event : null,
    );

    async function startSync(): Promise<void> {
        if (selectedConnectionId === '') return;
        busy = true;
        try {
            await controller.startProviderModelSync(selectedConnectionId);
        } finally {
            busy = false;
        }
    }

    async function refresh(jobId: string): Promise<void> {
        busy = true;
        try {
            await controller.refreshProviderModelSync(jobId);
        } finally {
            busy = false;
        }
    }

    function terminal(state: string): boolean {
        return ['completed', 'failed', 'cancelled'].includes(state);
    }
</script>

<section class="workflow-section" aria-labelledby="model-sync-title">
    <header class="workflow-heading">
        <div>
            <p class="eyebrow">Durable model metadata job</p>
            <h2 id="model-sync-title">{$tr('model_sync.title')}</h2>
            <p>
                {$tr('model_sync.hint')}
            </p>
        </div>
    </header>

    <form
        class="start-row"
        aria-label={$tr('model_sync.start.label')}
        onsubmit={(event) => {
            event.preventDefault();
            void startSync();
        }}
    >
        <label>
            <span>{$tr('model_sync.connection')}</span>
            <select bind:value={selectedConnectionId} required>
                <option value="">{$tr('model_sync.select')}</option>
                {#each workspace.connections as connection (connection.id)}
                    <option value={connection.id}>{connection.display_name}</option>
                {/each}
            </select>
        </label>
        <button class="primary" type="submit" disabled={busy || selectedConnectionId === ''}>
            {$tr('model_sync.start.label')}
        </button>
    </form>

    {#if workspace.model_sync_jobs.length > 0}
        <div class="job-picker">
            <label>
                <span>{$tr('model_sync.saved_jobs')}</span>
                <select
                    value={workspace.selected_model_sync_job_id ?? ''}
                    onchange={(event) => {
                        const id = event.currentTarget.value;
                        if (id !== '') void refresh(id);
                    }}
                >
                    <option value="">{$tr('model_sync.select')}</option>
                    {#each workspace.model_sync_jobs as job (job.id)}
                        <option value={job.id}>
                            {job.connection_id} · {job.state} · r{job.revision}
                        </option>
                    {/each}
                </select>
            </label>
            <button
                type="button"
                disabled={selectedJob === null || busy}
                onclick={() => {
                    if (selectedJob) void refresh(selectedJob.id);
                }}
            >
                {$tr('model_sync.refresh_events')}
            </button>
        </div>
    {/if}

    {#if selectedJob}
        <article class="job-card">
            <header>
                <div>
                    <h3>{selectedJob.connection_id}</h3>
                    <p>{selectedJob.state} · revision {selectedJob.revision}</p>
                </div>
                <button
                    class="danger"
                    type="button"
                    disabled={busy || terminal(selectedJob.state)}
                    onclick={() => void controller.cancelProviderModelSync(selectedJob.id)}
                >
                    {$tr('model_sync.cancel')}
                </button>
            </header>

            {#if selectedEvent}
                <div class="progress-block">
                    <strong>{selectedEvent.progress.message_key}</strong>
                    <progress
                        max={Math.max(1, selectedEvent.progress.total_steps)}
                        value={selectedEvent.progress.completed_steps}
                    ></progress>
                    <span>
                        {selectedEvent.progress.completed_steps} /
                        {selectedEvent.progress.total_steps}
                    </span>
                </div>
            {/if}

            {#if selectedJob.failure}
                <p class="failure" role="alert">
                    {selectedJob.failure.message_key} ({selectedJob.failure.code})
                </p>
            {/if}

            {#if selectedJob.review}
                {@const review = selectedJob.review}
                <section class="review-card" aria-labelledby="model-sync-review-title">
                    <h4 id="model-sync-review-title">{$tr('model_sync.review.title')}</h4>
                    <dl>
                        <div>
                            <dt>{$tr('model_sync.review.new')}</dt>
                            <dd>
                                {$tr('model_sync.review.count', {
                                    count: review.diff.newly_seen_model_route_ids.length,
                                })}
                            </dd>
                        </div>
                        <div>
                            <dt>{$tr('model_sync.review.missing')}</dt>
                            <dd>
                                {$tr('model_sync.review.count', {
                                    count: review.diff.missing_model_route_ids.length,
                                })}
                            </dd>
                        </div>
                        <div>
                            <dt>{$tr('model_sync.review.initial_presets')}</dt>
                            <dd>
                                {$tr('model_sync.review.count', {
                                    count: review.diff.initial_presets.length,
                                })}
                            </dd>
                        </div>
                        <div>
                            <dt>{$tr('model_sync.review.needs_preset')}</dt>
                            <dd>
                                {$tr('model_sync.review.count', {
                                    count: review.diff.routes_requiring_preset_configuration.length,
                                })}
                            </dd>
                        </div>
                    </dl>
                    <p>
                        {$tr('model_sync.review.source', { source: review.diff.provenance.source })}
                        {review.diff.provenance.endpoint_path} ·
                        {review.diff.provenance.pages_fetched} page
                    </p>
                    <code>{review.sha256}</code>
                    <button
                        class="primary"
                        type="button"
                        disabled={busy || selectedJob.state !== 'diff-ready-awaiting-review'}
                        onclick={() => void controller.approveProviderModelSync(selectedJob.id)}
                    >
                        {$tr('model_sync.review.apply')}
                    </button>
                </section>
            {/if}

            {#if selectedJob.state === 'interrupted'}
                <p class="notice">
                    {$tr('model_sync.interrupted')}
                </p>
            {/if}
        </article>
    {/if}
</section>

<style>
    .workflow-section {
        padding: 20px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-raised);
    }

    .workflow-heading h2,
    .job-card h3,
    .review-card h4 {
        margin: 3px 0;
    }

    .workflow-heading p:last-child,
    .job-card header p,
    .review-card p,
    .notice {
        margin: 5px 0 0;
        color: var(--ink-muted);
        line-height: 1.45;
    }

    .start-row,
    .job-picker,
    .job-card > header {
        display: flex;
        gap: 12px;
        align-items: end;
        justify-content: space-between;
    }

    .start-row,
    .job-picker {
        margin-top: 16px;
    }

    label {
        display: grid;
        flex: 1;
        gap: 6px;
        color: var(--ink-muted);
        font-size: 0.78rem;
        font-weight: 700;
    }

    .job-card {
        margin-top: 16px;
        padding: 16px;
        border: 1px solid var(--line);
        border-radius: 14px;
        background: var(--surface);
    }

    .progress-block {
        display: grid;
        grid-template-columns: auto 1fr auto;
        gap: 10px;
        align-items: center;
        margin-top: 14px;
    }

    progress {
        width: 100%;
    }

    .review-card,
    .failure,
    .notice {
        margin-top: 14px;
        padding: 12px;
        border-radius: 12px;
        background: var(--surface-sunken);
    }

    .failure {
        color: var(--danger);
    }

    .review-card dl {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 8px;
    }

    .review-card dl > div {
        padding: 9px;
        border-radius: 9px;
        background: var(--surface);
    }

    dt {
        color: var(--ink-muted);
        font-size: 0.7rem;
    }

    dd {
        margin: 3px 0 0;
        font-weight: 800;
    }

    code {
        display: block;
        margin: 10px 0;
        overflow-wrap: anywhere;
        font-size: 0.72rem;
    }

    @media (max-width: 640px) {
        .start-row,
        .job-picker,
        .job-card > header {
            align-items: stretch;
            flex-direction: column;
        }

        .review-card dl {
            grid-template-columns: 1fr;
        }
    }
</style>
