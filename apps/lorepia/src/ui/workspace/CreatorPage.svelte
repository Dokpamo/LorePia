<script lang="ts">
    import { ArrowLeft, PanelsTopLeft } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import IconButton from './IconButton.svelte';
    let { onback, showBack = true }: { onback: () => void; showBack?: boolean } = $props();
</script>

<header class="ui-page-header" class:ui-navigation-header={showBack}>
    {#if showBack}<IconButton label={$tr('uiPreview.backChat')} onclick={onback}
            ><ArrowLeft /></IconButton
        >{/if}
    <div class="ui-title-group">
        <strong>{$tr('uiPreview.subpage')}</strong><small>{$tr('uiPreview.creatorSpace')}</small>
    </div>
</header>
<div class="ui-sub-body">
    <div class="ui-placeholder">
        <PanelsTopLeft aria-hidden="true" /><strong>{$tr('uiPreview.creatorContent')}</strong><small
            >{$tr('uiPreview.freeLayout')}</small
        >
    </div>
    <div class="ui-sub-skeleton" aria-hidden="true"><span></span><span></span></div>
</div>
