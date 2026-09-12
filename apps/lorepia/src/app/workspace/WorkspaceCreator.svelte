<script lang="ts">
    import { ArrowLeft } from '@lucide/svelte';
    import type { PortableRuntimeLifecycle } from '../../features/chat/portable-runtime-lifecycle.svelte';
    import PortableRuntimeControls from './runtime/RuntimeControls.svelte';
    import PortableMessage from '../../features/chat/PortableMessage.svelte';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import { tr } from '../../lib/i18n';
    import IconButton from '../../ui/workspace/IconButton.svelte';
    let {
        runtime,
        client,
        onback,
    }: { runtime: PortableRuntimeLifecycle; client: LorepiaClient; onback: () => void } = $props();
</script>

<header class="ui-page-header ui-navigation-header">
    <IconButton label={$tr('uiPreview.backChat')} onclick={onback}><ArrowLeft /></IconButton>
    <div class="ui-title-group"><strong>{$tr('chat.runtime.roomTitle')}</strong></div>
</header>
<div class="ui-card-room">
    {#if runtime.profileLoading}
        <p class="ui-card-room-status" role="status">{$tr('chat.runtime.profileLoading')}</p>
    {:else if runtime.profileError}
        <div class="ui-card-room-status" role="alert">
            <p>{runtime.profileError}</p>
            <button class="ui-submit ui-pressable" onclick={() => runtime.retryProfile()}>
                <span class="ui-press-visual">{$tr('chat.runtime.retryProfile')}</span>
            </button>
        </div>
    {:else if runtime.profile}
        {#if runtime.error}<p class="ui-card-room-status" role="alert">{runtime.error}</p>{/if}
        {#if !runtime.displayApproved || !runtime.canReadChat}
            <section class="ui-card-room-status">
                <p>{$tr('chat.runtime.enableDisplayHint')}</p>
                <button
                    class="ui-submit ui-pressable"
                    disabled={runtime.phase === 'loading'}
                    onclick={() => void runtime.approveDisplay()}
                >
                    <span class="ui-press-visual">{$tr('chat.runtime.enableDisplay')}</span>
                </button>
            </section>
        {/if}
        <details class="ui-card-runtime-controls">
            <summary>{$tr('chat.runtime.advancedControls')}</summary>
            <PortableRuntimeControls {runtime} />
        </details>
        {#if runtime.displayApproved && runtime.canReadChat}
            <div class="ui-card-room-content">
                <PortableMessage
                    surface="room"
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
    {/if}
</div>

<style>
    .ui-card-room {
        display: flex;
        flex: 1;
        flex-direction: column;
        min-height: 0;
        overflow: auto;
    }
    .ui-card-room-status {
        padding: 16px 24px;
    }
    .ui-card-room-status p {
        color: var(--color-text-muted);
        line-height: 1.6;
    }
    .ui-card-runtime-controls {
        flex: none;
        padding: 8px 24px;
    }
    .ui-card-runtime-controls summary {
        cursor: pointer;
        font-size: 14px;
        padding: 8px 0;
    }
    .ui-card-room-content {
        flex: 1;
        min-height: 360px;
        overflow: hidden;
    }
</style>
