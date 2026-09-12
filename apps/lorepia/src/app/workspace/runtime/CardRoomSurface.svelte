<script lang="ts">
    import type { PortableRuntimeLifecycle } from '../../../features/chat/portable-runtime-lifecycle.svelte';
    import PortableMessage from '../../../features/chat/PortableMessage.svelte';
    import type { LorepiaClient } from '../../../lib/ipc/contracts';
    let {
        runtime,
        client,
        floating = false,
    }: { runtime: PortableRuntimeLifecycle; client: LorepiaClient; floating?: boolean } = $props();
</script>

{#if runtime.profile && runtime.displayApproved && runtime.canReadChat}
    <div class="ui-card-surface" class:ui-card-floating={floating}>
        <PortableMessage
            surface="room"
            {floating}
            text={runtime.lastCharacterMessage || '※※'}
            {client}
            profile={runtime.profile}
            enabled={true}
            variables={runtime.variables}
            backgroundMarkup={runtime.background}
            lastCharacterMessage={runtime.lastCharacterMessage}
            messageIndex={runtime.lastMessageIndex}
            lastMessageId={runtime.lastMessageIndex}
            onAction={(action: string) => void runtime.handleAction(action)}
        />
    </div>
{/if}

<style>
    .ui-card-surface {
        flex: 1;
        min-height: 0;
        height: 100%;
        overflow: clip;
    }
    .ui-card-floating {
        position: absolute;
        inset: var(--ui-header) 0
            calc(var(--ui-composer-height, 64px) + var(--ui-composer-bottom, 12px));
        height: auto;
        z-index: 14;
        pointer-events: none;
    }
    .ui-card-floating :global(iframe) {
        pointer-events: auto;
    }
</style>
