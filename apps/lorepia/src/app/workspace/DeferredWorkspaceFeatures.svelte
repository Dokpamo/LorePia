<script lang="ts" module>
    import type WorkspaceFeatures from './WorkspaceFeatures.svelte';
    let request: Promise<typeof WorkspaceFeatures> | null = null;
    let loaded: typeof WorkspaceFeatures | null = null;
    function loadFeatures() {
        request ??= import('./WorkspaceFeatures.svelte')
            .then((module) => (loaded = module.default))
            .catch((error: unknown) => {
                request = null;
                throw error;
            });
        return request;
    }
</script>

<script lang="ts">
    import { onMount, type ComponentProps } from 'svelte';
    import { tr } from '../../lib/i18n';
    import SettingsPanel from '../../ui/workspace/SettingsPanel.svelte';
    import LoadingState from '../../ui/workspace/LoadingState.svelte';
    const props: ComponentProps<typeof WorkspaceFeatures> = $props();
    let Features = $state<typeof WorkspaceFeatures | null>(loaded);
    let failed = $state(false);
    let busy = false;
    let mounted = false;
    async function load() {
        if (busy) return;
        busy = true;
        failed = false;
        try {
            const component = await loadFeatures();
            if (mounted) Features = component;
        } catch {
            if (mounted) failed = true;
        } finally {
            busy = false;
        }
    }
    onMount(() => {
        mounted = true;
        if (!Features) void load();
        return () => {
            mounted = false;
        };
    });
</script>

{#if Features}<Features {...props} />
{:else}<SettingsPanel
        title={$tr(failed ? 'ux.loading.settingsFailed' : 'ux.loading.settings')}
        root={props.root}
        onclose={props.onclose}
    >
        {#if failed}<div role="alert">
                <button class="ui-result-action" onclick={() => void load()}
                    >{$tr('workspace.retry')}</button
                >
            </div>
        {:else}<LoadingState label={$tr('ux.loading.settings')} />{/if}
    </SettingsPanel>
{/if}
