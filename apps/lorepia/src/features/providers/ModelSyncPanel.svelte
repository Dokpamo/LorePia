<script lang="ts">
    import { ListPlus } from '@lucide/svelte';
    import { tick } from 'svelte';
    import ChoiceField from '../../components/ChoiceField.svelte';
    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import DetailPage from '../../components/detail/DetailPage.svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import type { ModelSyncJobDto } from '../../lib/ipc/contracts';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        nestedPage?: string | null;
        nestedTitle?: string;
    }

    let {
        appState,
        controller,
        nestedPage = $bindable(null),
        nestedTitle = $bindable(''),
    }: Props = $props();
    let selectedConnectionId = $state('');
    let busy = $state(false);

    const workspace = $derived(appState.providers.workspace);
    const selectedJobId = $derived(
        nestedPage?.startsWith('job:') ? nestedPage.slice('job:'.length) : null,
    );
    const selectedJob = $derived(
        workspace.model_sync_jobs.find((job) => job.id === selectedJobId) ?? null,
    );
    const selectedEvent = $derived(
        workspace.model_sync_event?.job_id === selectedJob?.id ? workspace.model_sync_event : null,
    );
    const selectedConnectionExists = $derived(
        workspace.connections.some((connection) => connection.id === selectedConnectionId),
    );

    function connectionName(connectionId: string): string {
        return (
            workspace.connections.find((connection) => connection.id === connectionId)
                ?.display_name ?? connectionId
        );
    }

    async function run(action: () => Promise<void>): Promise<void> {
        if (busy) return;
        busy = true;
        try {
            await action();
        } finally {
            busy = false;
        }
    }

    async function startSync(): Promise<void> {
        if (busy || !selectedConnectionExists) return;
        const previousJobId = workspace.selected_model_sync_job_id;
        await run(() => controller.startProviderModelSync(selectedConnectionId));
        await tick();
        const jobId = workspace.selected_model_sync_job_id;
        const job = workspace.model_sync_jobs.find((candidate) => candidate.id === jobId);
        if (jobId !== null && jobId !== previousJobId && job) {
            nestedTitle = $tr('settings.page.discovery.sync_job');
            nestedPage = `job:${jobId}`;
        }
    }

    function beginCreate(): void {
        selectedConnectionId = '';
        nestedTitle = $tr('settings.page.discovery.sync_create');
        nestedPage = 'create';
    }

    function openJob(job: ModelSyncJobDto): void {
        nestedTitle = $tr('settings.page.discovery.sync_job');
        nestedPage = `job:${job.id}`;
        void run(() => controller.refreshProviderModelSync(job.id));
    }

    function terminal(state: string): boolean {
        return ['completed', 'failed', 'cancelled'].includes(state);
    }

    $effect(() => {
        if (selectedConnectionId !== '' && !selectedConnectionExists) selectedConnectionId = '';
    });

    $effect(() => {
        const page = nestedPage;
        if (page === null) {
            if (nestedTitle !== '') nestedTitle = '';
            return;
        }

        if (page === 'create') {
            const title = $tr('settings.page.discovery.sync_create');
            if (nestedTitle !== title) nestedTitle = title;
            return;
        }

        if (!page.startsWith('job:') || selectedJob === null) {
            nestedPage = null;
            nestedTitle = '';
            return;
        }

        const title = $tr('settings.page.discovery.sync_job');
        if (nestedTitle !== title) nestedTitle = title;
    });
</script>

{#snippet detailContent()}
    {#if nestedPage === 'create'}
        <form
            id="model-sync-start-form"
            class="sync-start-form"
            aria-label={$tr('model_sync.start.label')}
            onsubmit={(event) => {
                event.preventDefault();
                void startSync();
            }}
        >
            <ChoiceField
                id="model-sync-connection"
                label={$tr('model_sync.connection')}
                value={selectedConnectionId}
                options={[
                    { value: '', label: $tr('model_sync.select') },
                    ...workspace.connections.map((connection) => ({
                        value: connection.id,
                        label: connection.display_name,
                    })),
                ]}
                onSelect={(value: string) => (selectedConnectionId = value)}
                required
                disabled={busy}
            />
        </form>

        {#if workspace.connections.length === 0}
            <p class="inline-note">먼저 프로바이더 연결을 추가해 주세요.</p>
        {/if}
    {:else if nestedPage === null}
        {#if workspace.model_sync_jobs.length === 0}
            <p class="inline-note">저장된 동기화 작업이 없습니다.</p>
        {:else}
            <ul class="setting-list sync-job-list" aria-label={$tr('model_sync.saved_jobs')}>
                {#each workspace.model_sync_jobs as job (job.id)}
                    <li>
                        <button
                            class="setting-row sync-job-row"
                            type="button"
                            disabled={busy}
                            onclick={() => openJob(job)}
                        >
                            <span class="setting-content">
                                <span class="setting-copy sync-job-copy">
                                    <strong>{connectionName(job.connection_id)}</strong>
                                    <small>{job.state} · r{job.revision}</small>
                                </span>
                                <span class="sync-job-updated">{job.updated_at}</span>
                            </span>
                        </button>
                    </li>
                {/each}
            </ul>
        {/if}
    {:else if selectedJob}
        <article
            class="sync-job-detail"
            aria-label={$tr('model_sync.job.label', {
                connection: connectionName(selectedJob.connection_id),
            })}
        >
            <dl class="detail-fields">
                <div>
                    <dt>{$tr('model_sync.connection')}</dt>
                    <dd>{connectionName(selectedJob.connection_id)}</dd>
                </div>
                <div>
                    <dt>상태</dt>
                    <dd>{selectedJob.state}</dd>
                </div>
                <div>
                    <dt>리비전</dt>
                    <dd>{selectedJob.revision}</dd>
                </div>
                <div>
                    <dt>최근 갱신</dt>
                    <dd>{selectedJob.updated_at}</dd>
                </div>
            </dl>

            {#if selectedEvent}
                <section class="progress-section" aria-label="동기화 진행 상황">
                    <div class="progress-copy">
                        <strong>{selectedEvent.progress.message_key}</strong>
                        <span>
                            {selectedEvent.progress.completed_steps} /
                            {selectedEvent.progress.total_steps}
                        </span>
                    </div>
                    <progress
                        max={Math.max(1, selectedEvent.progress.total_steps)}
                        value={selectedEvent.progress.completed_steps}
                    ></progress>
                </section>
            {/if}

            {#if selectedJob.failure}
                <p class="failure" role="alert">
                    {selectedJob.failure.message_key} ({selectedJob.failure.code})
                </p>
            {/if}

            {#if selectedJob.review}
                {@const review = selectedJob.review}
                <section class="review-section" aria-labelledby="model-sync-review-title">
                    <h2 id="model-sync-review-title">{$tr('model_sync.review.title')}</h2>
                    <dl class="detail-fields review-fields">
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
                        <div>
                            <dt>출처</dt>
                            <dd>
                                {review.diff.provenance.source} ·
                                {review.diff.provenance.endpoint_path} ·
                                {review.diff.provenance.pages_fetched} page
                            </dd>
                        </div>
                        <div>
                            <dt>SHA-256</dt>
                            <dd><code>{review.sha256}</code></dd>
                        </div>
                    </dl>
                </section>
            {/if}

            {#if selectedJob.state === 'interrupted'}
                <p class="notice">{$tr('model_sync.interrupted')}</p>
            {/if}
        </article>
    {/if}
{/snippet}

{#snippet detailActions()}
    {#if nestedPage === null}
        <DetailActionBar className="model-sync-action-bar" ariaLabel="모델 동기화 목록 작업">
            <button
                class="primary detail-action detail-action--wide"
                type="button"
                disabled={busy}
                onclick={beginCreate}
            >
                <ListPlus aria-hidden="true" />
                새 모델 동기화
            </button>
        </DetailActionBar>
    {:else if nestedPage === 'create'}
        <DetailActionBar
            className="model-sync-action-bar"
            ariaLabel={$tr('model_sync.start.label')}
        >
            <button
                class="primary detail-action detail-action--wide"
                type="submit"
                form="model-sync-start-form"
                disabled={busy || !selectedConnectionExists}
            >
                <ListPlus aria-hidden="true" />
                {$tr('model_sync.start.label')}
            </button>
        </DetailActionBar>
    {:else if selectedJob && !terminal(selectedJob.state)}
        <DetailActionBar className="model-sync-action-bar" ariaLabel="동기화 작업">
            <button
                class="danger detail-action detail-action--destructive detail-action--borderless"
                type="button"
                disabled={busy}
                onclick={() => void run(() => controller.cancelProviderModelSync(selectedJob.id))}
            >
                {$tr('model_sync.cancel')}
            </button>
            {#if selectedJob.review}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy || selectedJob.state !== 'diff-ready-awaiting-review'}
                    onclick={() =>
                        void run(() => controller.approveProviderModelSync(selectedJob.id))}
                >
                    {$tr('model_sync.review.apply')}
                </button>
            {:else}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy}
                    onclick={() =>
                        void run(() => controller.refreshProviderModelSync(selectedJob.id))}
                >
                    {$tr('model_sync.refresh_events')}
                </button>
            {/if}
        </DetailActionBar>
    {/if}
{/snippet}

<DetailPage
    className="model-sync-panel"
    scrollClassName="provider-scroll settings-detail-scroll model-sync-scroll"
    ariaLabel={$tr('model_sync.title')}
    resetKey={nestedPage ?? 'index'}
    hasActions={nestedPage === null ||
        nestedPage === 'create' ||
        (selectedJob !== null && !terminal(selectedJob.state))}
    content={detailContent}
    actions={detailActions}
/>

<style>
    .sync-start-form,
    .sync-job-detail,
    .review-section,
    .progress-section {
        display: grid;
        min-width: 0;
        gap: 16px;
    }

    .inline-note,
    .failure,
    .notice {
        padding: 12px;
        border-radius: 12px;
        margin: 0;
        background: var(--surface-sunken);
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        line-height: 1.5;
    }

    .failure {
        border: 1px solid var(--status-error-border);
        color: var(--status-error-fg);
        background: var(--status-error-bg);
    }

    .sync-job-list {
        width: auto;
        margin: 0;
    }

    .sync-job-copy {
        display: grid;
        min-width: 0;
        gap: 5px;
    }

    .sync-job-copy > :is(strong, small),
    .sync-job-updated {
        overflow: hidden;
        font-size: var(--detail-support-type);
        line-height: 1.35;
        text-overflow: ellipsis;
    }

    .sync-job-copy > strong {
        color: var(--ink);
        font-weight: 550;
        white-space: nowrap;
    }

    .sync-job-copy > small,
    .sync-job-updated {
        color: var(--ink-muted);
        font-weight: 500;
    }

    .sync-job-updated {
        max-width: 42%;
        flex: none;
        text-align: right;
        white-space: nowrap;
    }

    .detail-fields {
        display: grid;
        margin: 0;
    }

    .detail-fields > div {
        display: grid;
        grid-template-columns: minmax(112px, 0.65fr) minmax(0, 1.35fr);
        align-items: start;
        padding: 13px 2px;
        border-bottom: 1px solid var(--line);
        gap: 12px;
    }

    .detail-fields > div:first-child {
        padding-top: 0;
    }

    dt {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    dd {
        min-width: 0;
        margin: 0;
        color: var(--ink);
        font-size: var(--detail-support-type);
        font-weight: 650;
        overflow-wrap: anywhere;
    }

    code {
        font-size: 0.76rem;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
    }

    .progress-section,
    .review-section {
        padding-top: 18px;
        border-top: 1px solid var(--line);
    }

    .progress-copy {
        display: flex;
        align-items: center;
        justify-content: space-between;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        gap: 12px;
    }

    progress {
        width: 100%;
        accent-color: var(--accent);
    }

    .review-section h2 {
        margin: 0;
        color: var(--ink);
        font-size: var(--detail-support-type);
        font-weight: 700;
        line-height: 1.35;
    }

    :global(.model-sync-action-bar svg) {
        width: 20px;
        height: 20px;
        flex: none;
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 1.8;
    }

    @container view (max-width: 640px) {
        .detail-fields > div {
            grid-template-columns: 1fr;
            gap: 5px;
        }

        .sync-job-updated {
            display: none;
        }
    }
</style>
