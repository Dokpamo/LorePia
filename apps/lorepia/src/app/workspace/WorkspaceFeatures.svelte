<script lang="ts">
    import { ArrowLeft } from '@lucide/svelte';
    import { onMount, untrack } from 'svelte';
    import type { LorepiaAppController, LorepiaAppState } from '../app-controller';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { PersonaClientApi } from '../../features/personas/persona-contracts';
    import { tr } from '../../lib/i18n';
    import ProviderSettings from '../../features/providers/ProviderSettings.svelte';
    import { settingsParent, type SettingsSection } from '../../features/providers/settings-contracts';
    import { PersonaController } from '../../features/personas/persona-controller';
    import OrchestrationStudio from '../../features/orchestration/OrchestrationStudio.svelte';
    import { OrchestrationController } from '../../features/orchestration/orchestration-controller';
    import { ContentPackageController } from '../../features/orchestration/content-package-controller';
    import { studioDetailParent, type StudioSection } from '../../features/orchestration/studio-contracts';
    import IconButton from '../../ui/workspace/IconButton.svelte';
    import { edgeBack, requestBack } from '../../ui/workspace/edge-back';
    import { pageSlide } from '../../ui/workspace/navigation-motion';
    let {
        client,
        appState,
        controller,
        mode,
        onclose,
    }: {
        client: LorepiaClient;
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        mode: 'providers' | 'studio';
        onclose: () => void;
    } = $props();
    const personas = untrack(
        () => new PersonaController(client as LorepiaClient & Partial<PersonaClientApi>),
    );
    const orchestration = untrack(() => new OrchestrationController(client));
    const packages = untrack(() => new ContentPackageController(client));
    const personaStore = personas.state;
    const orchestrationStore = orchestration.state;
    const packageStore = packages.state;
    let section = $state<SettingsSection | null>(null);
    let detail = $state<string | null>(null);
    let studio = $state<StudioSection | null>(null);
    let panel: HTMLElement;
    $effect(() => {
        if (appState.bootstrap.phase === 'ready')
            void personas.loadContext(appState.selected_conversation?.id ?? null);
    });
    $effect(() => {
        if (appState.bootstrap.phase === 'ready')
            void orchestration.loadContext(
                appState.selected_conversation?.id ?? null,
                appState.conversation_state?.active_branch_id ?? null,
            );
    });
    onMount(() => {
        void packages.loadPendingImports();
        return () => {
            personas.destroy();
            orchestration.destroy();
            packages.destroy();
        };
    });
    function back() {
        if (detail) detail = studioDetailParent(detail);
        else if (section) section = settingsParent(section);
        else if (studio) studio = null;
        else onclose();
    }
</script>

<div
    class="ui-overlay ui-live-features"
    transition:pageSlide
    bind:this={panel}
    role="dialog"
    aria-label={$tr('workspace.settings')}
    aria-modal="true"
    tabindex="-1"
    use:edgeBack={{ onback: back }}
>
    <header class="ui-page-header ui-navigation-header">
        <IconButton label={$tr('workspace.back')} onclick={() => requestBack(panel)}
            ><ArrowLeft /></IconButton
        >
        <h1>
            {$tr(mode === 'providers' && !studio ? section ? `settings.section.${section}.title` : 'settingsUi.title' : 'workspace.advanced')}
        </h1>
    </header>
    <div class="ui-feature-content" data-ui-no-swipe>
        {#if mode === 'providers' && !studio}
            <ProviderSettings
                {appState}
                {controller}
                services={{
                    client, appState, appController: controller,
                    orchestrationState: $orchestrationStore,
                    orchestrationController: orchestration,
                    contentPackageState: $packageStore,
                    contentPackageController: packages,
                }}
                personaController={personas}
                personaState={$personaStore}
                {section}
                bind:detailPage={detail}
                onOpenSection={(value) => (section = value)}
                onOpenStudio={(value) => (studio = value)}
            />
        {:else}
            <OrchestrationStudio
                {client}
                {appState}
                controller={orchestration}
                appController={controller}
                orchestrationState={$orchestrationStore}
                contentPackageState={$packageStore}
                contentPackageController={packages}
                section={studio}
                onOpenSection={(value) => (studio = value)}
            />
        {/if}
    </div>
</div>
