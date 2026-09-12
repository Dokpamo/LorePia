<script lang="ts">
    import { onMount, tick, untrack } from 'svelte';
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
    import type { BackDecision } from '../../../ui/workspace/edge-back';
    import PackageData from './PackageData.svelte';
    import SettingsRow from '../../../ui/workspace/SettingsRow.svelte';
    import DataAction from './DataAction.svelte';
    import ImportedSetup from './ImportedSetup.svelte';
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
    let importedSetup = $state(false);
    let importedEditor = $state<{ isBusy: () => boolean }>();
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
    let editor = $state<{
        isDirty: () => boolean;
        isBusy: () => boolean;
        discardChanges?: () => Promise<void>;
    }>();
    let confirming = $state(false);
    let resumeBack: (() => Promise<void>) | undefined;
    async function keepEditing() {
        confirming = false;
        await tick();
        await resumeBack?.();
        resumeBack = undefined;
    }
    function beforeback(): BackDecision {
        if (packageEditor?.isBusy() || editor?.isBusy()) return false;
        if (editor?.isDirty()) {
            return {
                confirm: (resume) => {
                    resumeBack = resume;
                    confirming = true;
                },
            };
        }
        return true;
    }
</script>

<SettingsPanel
    {title}
    {onclose}
    disabled={$controllerStore.busy}
    {beforeback}
    covered={confirming || packages || importedSetup}
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
            label={$tr(
                section === 'prompt' ? 'importSetup.promptTitle' : 'importSetup.memoryTitle',
            )}
            disabled={editor?.isDirty() ?? false}
            onclick={() => (importedSetup = true)}
        />
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

{#if importedSetup && (section === 'prompt' || section === 'memory')}
    <SettingsPanel
        title={$tr(section === 'prompt' ? 'importSetup.promptTitle' : 'importSetup.memoryTitle')}
        onclose={() => (importedSetup = false)}
        beforeback={() => !(importedEditor?.isBusy() ?? false)}
    >
        <ImportedSetup
            bind:this={importedEditor}
            kind={section === 'prompt' ? 'generation' : 'memory'}
            {services}
            documentsState={$controllerStore}
            onsaved={() => {
                importedSetup = false;
                void controller.load();
            }}
        />
    </SettingsPanel>
{/if}

{#if packages}
    <SettingsPanel
        title={$tr('workspaceData.pickPackage')}
        onclose={() => void closePackages()}
        beforeback={() => !(packageEditor?.isBusy() ?? false)}
    >
        <PackageData {services} bind:this={packageEditor} />
    </SettingsPanel>
{/if}

{#if confirming}<DiscardChanges
        onkeep={keepEditing}
        ondiscard={() => {
            void editor?.discardChanges?.();
            onclose();
        }}
    />{/if}
