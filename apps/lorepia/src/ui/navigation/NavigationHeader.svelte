<script lang="ts">
    import type { Snippet } from 'svelte';
    import { ArrowLeft } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import IconButton from '../workspace/IconButton.svelte';
    let { title, onback, actions }: { title: string; onback?: () => void; actions?: Snippet } =
        $props();
</script>

<header class="seed-header" class:seed-header-root={!onback} class:seed-header-detail={!!onback}>
    {#if onback}<IconButton label={$tr('uiPreview.back')} onclick={onback}><ArrowLeft /></IconButton
        >{/if}
    <h1>{title}</h1>
    {#if actions}<div class="seed-header-actions">{@render actions()}</div>{/if}
</header>
