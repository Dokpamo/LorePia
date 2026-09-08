<script lang="ts">
    import { onMount } from 'svelte';
    import { fade } from 'svelte/transition';
    import { tr } from '../../lib/i18n';
    import { trapFocus } from './focus-trap';

    let { onkeep, ondiscard }: { onkeep: () => void; ondiscard: () => void } = $props();
    const ids = $props.id();
    let keep: HTMLButtonElement;
    const duration = matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 140;
    onMount(() => keep.focus({ preventScroll: true }));
</script>

<div class="ui-confirm-layer" transition:fade={{ duration }} data-ui-no-swipe>
    <div
        class="ui-confirm"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={ids + '-title'}
        aria-describedby={ids + '-description'}
        tabindex="-1"
        onkeydown={(event) => {
            if (event.key === 'Escape') {
                event.preventDefault();
                event.stopPropagation();
                onkeep();
            }
            trapFocus(event);
        }}
    >
        <h2 id={ids + '-title'}>{$tr('uiPreview.unsavedTitle')}</h2>
        <p id={ids + '-description'}>{$tr('uiPreview.unsavedDescription')}</p>
        <div class="ui-confirm-actions">
            <button type="button" class="ui-discard ui-pressable" onclick={ondiscard}
                ><span class="ui-press-visual">{$tr('uiPreview.discardChanges')}</span></button
            >
            <button type="button" class="ui-submit ui-pressable" bind:this={keep} onclick={onkeep}
                ><span class="ui-press-visual">{$tr('uiPreview.keepEditing')}</span></button
            >
        </div>
    </div>
</div>
