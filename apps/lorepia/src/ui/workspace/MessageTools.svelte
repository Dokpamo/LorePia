<script lang="ts">
    import { Copy, EllipsisVertical, GitBranch, Pencil, RotateCcw } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import IconButton from './IconButton.svelte';

    let {
        user,
        busy,
        oncopy,
        onedit,
        onregenerate,
        onmenu,
    }: {
        user: boolean;
        busy: boolean;
        oncopy: () => void;
        onedit: () => void;
        onregenerate: () => void;
        onmenu: (view: 'actions' | 'branches', anchor: HTMLButtonElement) => void;
    } = $props();
</script>

<div class="ui-message-tools" role="group" aria-label={$tr('uiPreview.messageTools')}>
    <IconButton label={$tr('uiPreview.copyMessage')} onclick={oncopy}><Copy /></IconButton>
    {#if user}
        <IconButton label={$tr('uiPreview.editMessage')} disabled={busy} onclick={onedit}
            ><Pencil /></IconButton
        >
    {:else}
        <IconButton label={$tr('uiPreview.regenerate')} disabled={busy} onclick={onregenerate}
            ><RotateCcw /></IconButton
        >
    {/if}
    <IconButton
        label={$tr('uiPreview.branch')}
        disabled={busy}
        onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
            onmenu('branches', event.currentTarget)}><GitBranch /></IconButton
    >
    <IconButton
        label={$tr('uiPreview.messageMenu')}
        onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
            onmenu('actions', event.currentTarget)}><EllipsisVertical /></IconButton
    >
</div>
