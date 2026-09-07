<script lang="ts">
    import { onMount } from 'svelte';
    import type { LorepiaAppController, LorepiaAppState } from '../app-controller';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import { t } from '../../lib/i18n';
    import GenerationAttemptApprovals from '../../features/chat/GenerationAttemptApprovals.svelte';
    import MemoryQueryRetryPanel from '../../features/orchestration/MemoryQueryRetryPanel.svelte';
    import { InteractionRoomLifecycle } from '../../features/chat/interaction-room-lifecycle.svelte';
    import InteractionRoomSurface from '../../features/chat/InteractionRoomSurface.svelte';
    let {
        appState,
        controller,
        client,
        refreshEpoch,
        onnotice,
    }: {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        client: LorepiaClient;
        refreshEpoch: number;
        onnotice: (value: string) => void;
    } = $props();
    const interaction = new InteractionRoomLifecycle();
    $effect(() =>
        interaction.syncRoom(
            appState.selected_conversation?.id ?? null,
            appState.conversation_state?.active_branch_id ?? null,
        ),
    );
    onMount(() => interaction.mount(client));
</script>

{#if interaction.controller && interaction.state.phase !== 'unavailable'}<InteractionRoomSurface
        {client}
        controller={interaction.controller}
        state={interaction.state}
    />{/if}
<GenerationAttemptApprovals
    {client}
    conversationId={appState.selected_conversation?.id ?? null}
    sourceBranchId={appState.conversation_state?.active_branch_id ?? null}
    {refreshEpoch}
    hideWhenInactive
    onRetry={(id) =>
        onnotice(
            t(
                controller.stageGenerationAttemptRetry(id)
                    ? 'workspace.retryApproved'
                    : 'workspace.retryUnavailable',
            ),
        )}
/>
<MemoryQueryRetryPanel
    state={appState.memory_query_retries}
    {controller}
    headingId="workspace-memory-retry"
/>
