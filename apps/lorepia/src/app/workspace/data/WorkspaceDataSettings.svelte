<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import { tr } from '../../../lib/i18n';
    import type { SettingsServices } from '../../../features/providers/settings/settings-services';
    import type {
        PersonaController,
        PersonaState,
    } from '../../../features/personas/persona-controller';
    import { SettingsDocumentsController } from '../../../features/providers/settings/settings-documents';
    import SettingsPanel from '../../../ui/workspace/SettingsPanel.svelte';
    import PromptData from './PromptData.svelte';
    import MemoryData from './MemoryData.svelte';
    import PersonaData from './PersonaData.svelte';
    import StorageData from './StorageData.svelte';
    import PluginsData from './PluginsData.svelte';
    import DiscardChanges from '../../../ui/workspace/DiscardChanges.svelte';
    import PackageData from './PackageData.svelte';
    import SettingsRow from '../../../ui/workspace/SettingsRow.svelte';
    import DataAction from './DataAction.svelte';
    let {
        section,
        services,
        personas,
        personaState,
        onclose,
    }: {
        section: 'persona' | 'prompt' | 'memory' | 'plugins' | 'storage';
        services: SettingsServices;
        personas: PersonaController;
        personaState: PersonaState;
        onclose: () => void;
    } = $props();
    const controller = untrack(() => new SettingsDocumentsController(services.client));
    const controllerStore = controller.state;
    onMount(() => {
        if (section === 'prompt' || section === 'memory') void controller.load();
        return () => controller.destroy();
    });
    let packages = $state(false);
    let packageEditor = $state<{ isBusy: () => boolean }>();
    async function closePackages() {
        packages = false;
        if (section === 'prompt' || section === 'memory') await controller.load();
    }
    const title = $derived(
        $tr(
            section === 'persona'
                ? 'settingsUi.personas'
                : section === 'prompt'
                  ? 'settingsUi.prompts'
                  : section === 'memory'
                    ? 'settingsUi.memory'
                    : section === 'plugins'
                      ? 'settingsUi.plugins'
                      : 'settingsUi.storage',
        ),
    );
    let editor = $state<{ isDirty: () => boolean; isBusy: () => boolean }>();
    let confirming = $state(false);
    function beforeback() {
        if (packageEditor?.isBusy() || editor?.isBusy()) return false;
        if (editor?.isDirty()) {
            confirming = true;
            return false;
        }
        return true;
    }
</script>

<SettingsPanel
    {title}
    {onclose}
    disabled={$controllerStore.busy}
    {beforeback}
    covered={confirming || packages}
>
    {#if section === 'persona'}<PersonaData
            bind:this={editor}
            controller={personas}
            {personaState}
        />
    {:else if section === 'storage'}<StorageData {services} />
    {:else if section === 'plugins'}<SettingsRow
            label={$tr('workspaceData.pickPackage')}
            disabled={(editor?.isBusy() ?? false) || (editor?.isDirty() ?? false)}
            onclick={() => (packages = true)}
        /><PluginsData bind:this={editor} {services} />
    {:else if $controllerStore.phase === 'loading'}<p role="status">
            {$tr('settingsLive.loading')}
        </p>
    {:else if $controllerStore.phase === 'error'}<p role="alert">{$controllerStore.error}</p>
        <DataAction onclick={() => void controller.load()}>{$tr('settingsLive.retry')}</DataAction>
    {:else}
        <SettingsRow
            label={$tr('workspaceData.pickPackage')}
            disabled={editor?.isDirty() ?? false}
            onclick={() => (packages = true)}
        />
        {#if section === 'prompt'}<PromptData
                bind:this={editor}
                documentsState={$controllerStore}
                {controller}
                {services}
            />{:else}<MemoryData
                bind:this={editor}
                documentsState={$controllerStore}
                {controller}
                {services}
            />{/if}
        {#if $controllerStore.error}<p role="alert">{$controllerStore.error}</p>{/if}
        {#if $controllerStore.announcement}<p role="status">{$controllerStore.announcement}</p>{/if}
    {/if}
</SettingsPanel>

{#if packages}
    <SettingsPanel
        title={$tr('workspaceData.pickPackage')}
        onclose={() => void closePackages()}
        beforeback={() => !(packageEditor?.isBusy() ?? false)}
    >
        <PackageData {services} bind:this={packageEditor} />
    </SettingsPanel>
{/if}

{#if confirming}<DiscardChanges onkeep={() => (confirming = false)} ondiscard={onclose} />{/if}
