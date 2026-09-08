<script lang="ts">
    import type { LorepiaAppState, LorepiaAppController } from '../../app-controller';
    import { t, tr } from '../../../lib/i18n';
    import SettingsPanel from './AiPanel.svelte';
    import AiConnections from './AiConnections.svelte';
    import AiRoutes from './AiRoutes.svelte';
    import AiPresets from './AiPresets.svelte';
    import AiChoice from './AiChoice.svelte';
    import AiSwitch from './AiSwitch.svelte';
    import AiAction from './AiAction.svelte';
    import AiLink from './AiLink.svelte';
    import AiPreview from './AiPreview.svelte';
    import AiSync from './AiSync.svelte';
    import AiDiscovery from './AiDiscovery.svelte';
    import AiCatalog from './AiCatalog.svelte';
    import AiCapabilities from './AiCapabilities.svelte';
    import AiLegacy from './AiLegacy.svelte';
    let {
        appState,
        controller,
        onclose,
    }: { appState: LorepiaAppState; controller: LorepiaAppController; onclose: () => void } =
        $props();
    let page = $state('');
    let busy = $state(false);
    let targetDraft = $state<{ route: string; preset: string } | null>(null);
    let preserveDraft = $state<boolean | null>(null);
    const workspace = $derived(appState.providers.workspace);
    const saved = $derived({
        route: workspace.settings.selected_provider_profile_id
            ? ''
            : (workspace.settings.selected_model_route_id ?? ''),
        preset: workspace.settings.selected_provider_profile_id
            ? ''
            : (workspace.settings.selected_generation_preset_id ?? ''),
        preserve: workspace.settings.preserve_partial_generations,
    });
    const route = $derived((targetDraft ?? saved).route);
    const preset = $derived((targetDraft ?? saved).preset);
    const preserve = $derived(preserveDraft ?? saved.preserve);
    const targetDirty = $derived(route !== saved.route || preset !== saved.preset);
    const dirty = $derived(targetDirty || preserve !== saved.preserve);
    const routes = $derived(
        workspace.routes.filter(
            (r) => !workspace.legacy_profiles.some((p) => p.id === r.connection_id),
        ),
    );
    const presets = $derived(workspace.presets.filter((p) => p.model_route_id === route));
    $effect(() => {
        if (!targetDirty) targetDraft = null;
        if (preserve === saved.preserve) preserveDraft = null;
    });
    async function save() {
        if (busy) return;
        busy = true;
        try {
            if (
                !targetDirty ||
                (await controller.selectProviderGenerationTarget(route || null, preset || null))
            ) {
                if (await controller.setPreservePartialGenerations(preserve)) {
                    targetDraft = null;
                    preserveDraft = null;
                }
            }
        } finally {
            busy = false;
        }
    }
    function changeRoute(value: string) {
        if (value === route) return;
        targetDraft = {
            route: value,
            preset:
                value === saved.route
                    ? saved.preset
                    : (workspace.presets.find((p) => p.model_route_id === value)?.id ?? ''),
        };
    }
    async function preview() {
        if (busy) return;
        busy = true;
        try {
            await controller.previewSelectedProviderRequest();
        } finally {
            busy = false;
        }
    }
</script>

<SettingsPanel
    {appState}
    {controller}
    title={$tr('workspaceAi.title')}
    {onclose}
    disabled={busy}
    {dirty}
    covered={page !== ''}
>
    {#if appState.providers.phase !== 'ready'}<AiAction
            label={$tr('workspaceAi.reload')}
            onclick={() => void controller.loadProviders()}
        />{/if}
    <section class="ui-settings-group" inert={busy}>
        <h2>{$tr('workspaceAi.target')}</h2>
        <AiChoice
            label={$tr('workspaceAi.model')}
            value={route}
            options={[
                { value: '', label: t('workspaceAi.none') },
                ...routes.map((r) => ({ value: r.id, label: r.display_name ?? r.model_id })),
            ]}
            onselect={changeRoute}
        />
        <AiChoice
            label={$tr('workspaceAi.preset')}
            value={preset}
            options={[
                { value: '', label: t('workspaceAi.none') },
                ...presets.map((p) => ({ value: p.id, label: p.display_name })),
            ]}
            onselect={(value: string) => (targetDraft = { route, preset: value })}
        />
        <AiSwitch
            label={$tr('workspaceAi.partial')}
            checked={preserve}
            disabled={busy}
            onchange={(checked: boolean) => (preserveDraft = checked)}
        />
        <AiAction
            label={$tr('workspaceAi.save')}
            disabled={busy ||
                !dirty ||
                !(
                    (route === '' && preset === '') ||
                    (routes.some((r) => r.id === route) && presets.some((p) => p.id === preset))
                )}
            onclick={() => void save()}
        />
        <AiAction
            label={$tr('workspaceAi.preview')}
            secondary
            disabled={busy || dirty}
            onclick={() => void preview()}
        />
    </section>
    <section class="ui-settings-group" inert={busy}>
        <h2>{$tr('workspaceAi.manage')}</h2>
        <AiLink label={$tr('workspaceAi.connections')} onclick={() => (page = 'connections')} />
        <AiLink label={$tr('workspaceAi.model')} onclick={() => (page = 'routes')} />
        <AiLink label={$tr('workspaceAi.preset')} onclick={() => (page = 'presets')} />
    </section>
    <section class="ui-settings-group" inert={busy}>
        <h2>{$tr('workspaceAi.advanced')}</h2>
        <AiLink
            label={$tr('settings.page.discovery.provider')}
            onclick={() => (page = 'discovery')}
        />
        <AiLink label={$tr('settings.section.catalog.title')} onclick={() => (page = 'catalog')} />
        <AiLink label={$tr('settings.page.discovery.sync_job')} onclick={() => (page = 'sync')} />
        <AiLink label={$tr('workspaceAi.capabilities')} onclick={() => (page = 'capabilities')} />
        {#if workspace.legacy_profiles.length}
            <AiLink label={$tr('workspaceAi.legacy')} onclick={() => (page = 'legacy')} />
        {/if}
    </section>
    <AiPreview {appState} />
</SettingsPanel>

{#if page === 'connections'}<AiConnections {appState} {controller} onclose={() => (page = '')} />
{:else if page === 'routes'}<AiRoutes {appState} {controller} onclose={() => (page = '')} />
{:else if page === 'presets'}<AiPresets {appState} {controller} onclose={() => (page = '')} />
{:else if page === 'sync'}<AiSync {appState} {controller} onclose={() => (page = '')} />
{:else if page === 'discovery'}<AiDiscovery
        {appState}
        {controller}
        onclose={() => (page = '')}
    />{:else if page === 'catalog'}<AiCatalog
        {appState}
        {controller}
        onclose={() => (page = '')}
    />{:else if page === 'capabilities'}<AiCapabilities
        {appState}
        {controller}
        onclose={() => (page = '')}
    />{:else if page === 'legacy'}<AiLegacy {appState} {controller} onclose={() => (page = '')} />
{/if}
