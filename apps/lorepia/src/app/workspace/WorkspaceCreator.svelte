<script lang="ts">
    import { ArrowLeft, Settings } from '@lucide/svelte';
    import type { PortableRuntimeLifecycle } from '../../features/chat/portable-runtime-lifecycle.svelte';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import { tr } from '../../lib/i18n';
    import IconButton from '../../ui/workspace/IconButton.svelte';
    import CardRoomSurface from './runtime/CardRoomSurface.svelte';
    let {
        runtime,
        client,
        active,
        onback,
        onsettings,
    }: {
        runtime: PortableRuntimeLifecycle;
        client: LorepiaClient;
        active: boolean;
        onback: () => void;
        onsettings: (trigger: HTMLButtonElement) => void;
    } = $props();
</script>

<header class="ui-page-header ui-navigation-header">
    <IconButton label={$tr('uiPreview.backChat')} onclick={onback}><ArrowLeft /></IconButton>
    <div class="ui-title-group"><strong>{$tr('chat.runtime.roomTitle')}</strong></div>
    <IconButton
        label={$tr('uiPreview.roomSettings')}
        onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
            onsettings(event.currentTarget)}><Settings /></IconButton
    >
</header>
{#if active}
    {#if runtime.displayApproved && runtime.canReadChat}
        <CardRoomSurface {runtime} {client} />
    {:else}
        <div class="ui-card-room-status">
            <p role="status">
                {$tr(
                    runtime.profileLoading
                        ? 'chat.runtime.profileLoading'
                        : 'chat.runtime.settingsHint',
                )}
            </p>
            <button
                class="ui-submit ui-pressable"
                onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                    onsettings(event.currentTarget)}
            >
                <span class="ui-press-visual">{$tr('uiPreview.roomSettings')}</span>
            </button>
        </div>
    {/if}
{/if}

<style>
    .ui-card-room-status {
        padding: 16px 24px;
        color: var(--ui-secondary);
        line-height: 1.6;
    }
</style>
