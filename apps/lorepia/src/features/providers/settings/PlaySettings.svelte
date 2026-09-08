<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import { tr } from '../../../lib/i18n';
    import type { StudioSection } from '../../orchestration/studio-contracts';
    import type { SettingsSection } from '../settings-contracts';
    import type { SettingsServices } from './settings-services';
    import { SettingsDocumentsController, type SettingsDocumentsState } from './settings-documents';
    import MemorySettings from './MemorySettings.svelte';
    import PromptSettings from './PromptSettings.svelte';
    import StorageSettings from './StorageSettings.svelte';
    import ContentSection from '../../orchestration/studio/ContentSection.svelte';
    import ContentModuleLifecyclePanel from '../../orchestration/ContentModuleLifecyclePanel.svelte';
    let {
        section,
        services,
        onOpenStudio,
        detailPage = $bindable(null),
    }: {
        section: SettingsSection;
        services: SettingsServices;
        onOpenStudio: (section: StudioSection) => void;
        detailPage?: string | null;
    } = $props();
    const controller = untrack(() => new SettingsDocumentsController(services.client));
    let documentsState = $state<SettingsDocumentsState>({
        phase: 'loading',
        busy: false,
        error: null,
        announcement: '',
        prompts: [],
        memories: [],
        tasks: [],
    });
    onMount(() => {
        const unsubscribe = controller.state.subscribe((value) => (documentsState = value));
        if (section === 'prompt' || section === 'memory') void controller.load();
        return () => {
            unsubscribe();
            controller.destroy();
        };
    });
    const importPage = $derived(detailPage === 'packages');
    let wasImport = false;
    $effect(() => {
        if (!importPage && wasImport && (section === 'prompt' || section === 'memory'))
            void controller.load();
        wasImport = importPage;
    });
</script>

<div class="settings-workspace">
    {#if !importPage && (section === 'prompt' || section === 'memory')}<button
            class="settings-info-link"
            onclick={() => onOpenStudio(section)}>{$tr('settingsLive.openCreatorTools')}</button
        >{/if}
    {#if importPage}
        <ContentSection
            client={services.client}
            orchestrationState={services.orchestrationState}
            contentPackageState={services.contentPackageState}
            contentPackageController={services.contentPackageController}
            bind:detailPage
        />
    {:else if section === 'storage'}<StorageSettings {services} />
    {:else if section === 'plugins'}
        <p class="settings-lead">{$tr('settingsLive.pluginsIntro')}</p>
        <button class="settings-import-button" onclick={() => (detailPage = 'packages')}
            >{$tr('settingsUi.importPlugin')}</button
        >
        <ContentModuleLifecyclePanel
            client={services.client}
            conversationId={services.appState.selected_conversation?.id ?? null}
            branchId={services.appState.conversation_state?.active_branch_id ?? null}
            bind:detailPage
        />
    {:else}
        {#if documentsState.phase === 'loading'}<p role="status">{$tr('settingsLive.loading')}</p>
        {:else if documentsState.phase === 'error'}<p role="alert" class="error">
                {documentsState.error}
            </p>
            <button onclick={() => void controller.load()}>{$tr('settingsLive.retry')}</button>
        {:else}
            {#if section === 'memory'}<MemorySettings
                    {documentsState}
                    {controller}
                    {services}
                    onImport={() => (detailPage = 'packages')}
                />
            {:else}<PromptSettings
                    {documentsState}
                    {controller}
                    {services}
                    onImport={() => (detailPage = 'packages')}
                />{/if}
            {#if documentsState.error}<p role="alert" class="error">{documentsState.error}</p>{/if}
            {#if documentsState.announcement}<p class="settings-save-status" role="status">
                    {documentsState.announcement}
                </p>{/if}
        {/if}
    {/if}
</div>
