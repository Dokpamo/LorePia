<script lang="ts">
    import { tick } from 'svelte';
    import type { LorepiaAppState, LorepiaAppController } from '../../app-controller';
    import { tr } from '../../../lib/i18n';
    import SettingsPanel from './AiPanel.svelte';
    import AiChoice from './AiChoice.svelte';
    import AiAction from './AiAction.svelte';
    import AiLink from './AiLink.svelte';
    import AiReview from './AiReview.svelte';
    let {
        appState,
        controller,
        onclose,
    }: { appState: LorepiaAppState; controller: LorepiaAppController; onclose: () => void } =
        $props();
    let connection = $state('');
    let jobId = $state('');
    let busy = $state(false);
    const workspace = $derived(appState.providers.workspace);
    const job = $derived(workspace.model_sync_jobs.find((j) => j.id === jobId));
    async function run(action: () => Promise<void>) {
        if (busy) return;
        busy = true;
        try {
            await action();
        } finally {
            busy = false;
        }
    }
    async function start() {
        if (busy || !workspace.connections.some((item) => item.id === connection)) return;
        const previousJobId = workspace.selected_model_sync_job_id;
        await run(() => controller.startProviderModelSync(connection));
        await tick();
        const selectedId = workspace.selected_model_sync_job_id;
        if (
            selectedId !== null &&
            selectedId !== previousJobId &&
            workspace.model_sync_jobs.some((item) => item.id === selectedId)
        )
            jobId = selectedId;
    }
</script>

<SettingsPanel
    {appState}
    {controller}
    title={$tr('settings.page.discovery.sync_job')}
    disabled={busy}
    {onclose}
    covered={jobId !== ''}
>
    <section class="ui-settings-group" inert={busy}>
        <AiChoice
            label={$tr('model_sync.connection')}
            value={connection}
            options={workspace.connections.map((c) => ({ value: c.id, label: c.display_name }))}
            onselect={(v: string) => (connection = v)}
        />
        <AiAction
            label={$tr('settings.page.discovery.sync_create')}
            disabled={!workspace.connections.some((c) => c.id === connection)}
            onclick={() => void start()}
        />
        {#each workspace.model_sync_jobs as job (job.id)}<AiLink
                label={job.id + ' · ' + job.state}
                onclick={() => {
                    jobId = job.id;
                    void run(() => controller.refreshProviderModelSync(job.id));
                }}
            />{/each}
    </section>
</SettingsPanel>
{#if jobId !== ''}
    <SettingsPanel
        {appState}
        {controller}
        title={$tr('settings.page.discovery.sync_job')}
        disabled={busy}
        onclose={() => (jobId = '')}
    >
        <section class="ui-settings-group" inert={busy}>
            {#if job}
                <div class="ui-settings-row">
                    <span>{job.state}</span><small>{job.updated_at}</small>
                </div>
                {#if job.failure}<p class="ui-field-error" role="alert">
                        {job.failure.message_key}
                    </p>{/if}
                {#if job.review}
                    <h2>{$tr('model_sync.review.title')}</h2>
                    <AiReview label={$tr('model_sync.review.title')} value={job.review} />
                    {#each job.review.diff.listed_routes as route (route.id)}<div
                            class="ui-settings-row"
                        >
                            <span>{route.display_name ?? route.model_id}</span><small
                                >{route.status}</small
                            >
                        </div>{/each}
                    <div class="ui-settings-row">
                        <span>{$tr('model_sync.review.new')}</span><small
                            >{job.review.diff.newly_seen_model_route_ids.join(', ')}</small
                        >
                    </div>
                    <div class="ui-settings-row">
                        <span>{$tr('model_sync.review.missing')}</span><small
                            >{job.review.diff.missing_model_route_ids.join(', ')}</small
                        >
                    </div>
                    <AiAction
                        label={$tr('model_sync.review.title')}
                        disabled={job.state !== 'diff-ready-awaiting-review'}
                        onclick={() => void run(() => controller.approveProviderModelSync(job.id))}
                    />
                {/if}
                <AiAction
                    label={$tr('workspaceAi.reload')}
                    onclick={() => void run(() => controller.refreshProviderModelSync(job.id))}
                />
                <AiAction
                    label={$tr('uiPreview.cancel')}
                    disabled={['completed', 'failed', 'cancelled'].includes(job.state)}
                    onclick={() => void run(() => controller.cancelProviderModelSync(job.id))}
                />
            {/if}
        </section>
    </SettingsPanel>
{/if}
