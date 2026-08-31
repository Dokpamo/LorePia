<script lang="ts">
    import type { Snippet } from 'svelte';

    import type { LorepiaAppState } from '../../app/app-controller';
    import OrchestrationQuickDrawer from '../orchestration/OrchestrationQuickDrawer.svelte';
    import type {
        OrchestrationController,
        OrchestrationState,
    } from '../orchestration/orchestration-controller';

    interface Props {
        appState: LorepiaAppState;
        orchestrationState?: OrchestrationState;
        orchestrationController?: OrchestrationController;
        desktop: boolean;
        open?: boolean;
        view?: 'tools' | 'settings';
        onOpen: () => void;
        roomControls: Snippet<[closeSettings: () => Promise<void>]>;
    }

    let {
        appState,
        orchestrationState,
        orchestrationController,
        desktop,
        open = $bindable(false),
        view = $bindable('tools'),
        onOpen,
        roomControls,
    }: Props = $props();
</script>

{#if orchestrationState && orchestrationController}
    <OrchestrationQuickDrawer
        {appState}
        {orchestrationState}
        controller={orchestrationController}
        {desktop}
        bind:open
        bind:view
        {onOpen}
        {roomControls}
    />
{/if}
