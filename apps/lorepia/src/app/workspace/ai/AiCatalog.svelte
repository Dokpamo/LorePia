<script lang="ts">
    import { onMount } from 'svelte';
    import type { LorepiaAppState, LorepiaAppController } from '../../app-controller';
    import { tr } from '../../../lib/i18n';
    import SettingsPanel from './AiPanel.svelte';
    import AiChoice from './AiChoice.svelte';
    import AiAction from './AiAction.svelte';
    import AiReview from './AiReview.svelte';
    let {
        appState,
        controller,
        onclose,
    }: { appState: LorepiaAppState; controller: LorepiaAppController; onclose: () => void } =
        $props();
    let busy = $state(false);
    let revision = $state('');
    let from = $state('');
    let to = $state('');
    const workspace = $derived(appState.providers.workspace);
    const revisions = $derived(
        workspace.catalog_history?.revisions.map((r) => ({
            value: String(r.revision),
            label: String(r.revision),
        })) ?? [],
    );
    async function run(action: () => Promise<unknown>) {
        if (busy) return;
        busy = true;
        try {
            await action();
        } finally {
            busy = false;
        }
    }
    onMount(() => {
        void run(() => controller.loadProviderDiagnostics('catalog'));
    });
</script>

<SettingsPanel
    {appState}
    {controller}
    title={$tr('settings.section.catalog.title')}
    {onclose}
    disabled={busy}
>
    <section class="ui-settings-group" inert={busy}>
        {#if workspace.catalog_status}<AiReview
                label={$tr('workspaceAi.text119')}
                value={workspace.catalog_status}
            />{/if}
        <AiAction
            label={$tr('workspaceAi.text120')}
            onclick={() => void run(() => controller.pickProviderCatalogImport())}
        />
        {#if workspace.pending_catalog_import}
            <AiReview
                label={$tr('workspaceAi.text121')}
                value={workspace.pending_catalog_import.plan.review}
            />
            <AiAction
                label={$tr('workspaceAi.text122')}
                onclick={() => void run(() => controller.activateProviderCatalogImport())}
            />
            <AiAction
                label={$tr('uiPreview.cancel')}
                onclick={() => void run(() => controller.discardProviderCatalogImport())}
            />
        {/if}
        <AiChoice
            label={$tr('workspaceAi.text123')}
            value={revision}
            options={revisions}
            onselect={(v: string) => (revision = v)}
        />
        <AiAction
            label={$tr('workspaceAi.text124')}
            disabled={!revisions.some((r) => r.value === revision)}
            onclick={() =>
                void run(() => controller.prepareProviderCatalogRollback(Number(revision)))}
        />
        {#if workspace.pending_catalog_rollback}
            {@const plan = workspace.pending_catalog_rollback}
            <AiReview label={$tr('workspaceAi.text125')} value={plan} />
            <AiAction
                label={$tr('workspaceAi.text126')}
                onclick={() => void run(() => controller.activateProviderCatalogRollback(plan))}
            />
        {/if}
        <AiChoice
            label={$tr('workspaceAi.text127')}
            value={from}
            options={revisions}
            onselect={(v: string) => (from = v)}
        />
        <AiChoice
            label={$tr('workspaceAi.text128')}
            value={to}
            options={revisions}
            onselect={(v: string) => (to = v)}
        />
        <AiAction
            label={$tr('workspaceAi.text129')}
            disabled={!revisions.some((r) => r.value === from) ||
                !revisions.some((r) => r.value === to)}
            onclick={() =>
                void run(() => controller.diffProviderCatalogRevisions(Number(from), Number(to)))}
        />
        {#if workspace.catalog_diff}<AiReview
                label={$tr('workspaceAi.text130')}
                value={workspace.catalog_diff}
            />{/if}
    </section>
</SettingsPanel>
