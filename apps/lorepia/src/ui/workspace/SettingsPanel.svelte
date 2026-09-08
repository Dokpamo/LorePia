<script lang="ts">
    import { ArrowLeft } from '@lucide/svelte';
    import { onMount, tick, untrack, type Snippet } from 'svelte';
    import { tr } from '../../lib/i18n';
    import { edgeBack, requestBack } from './edge-back';
    import { trapFocus } from './focus-trap';
    import { pageSlide } from './navigation-motion';

    let {
        title,
        kind = 'app-settings',
        onclose,
        beforeback,
        disabled = false,
        covered = false,
        children,
    }: {
        title: string;
        kind?: string;
        onclose: () => void;
        beforeback?: () => boolean;
        disabled?: boolean;
        covered?: boolean;
        children: Snippet;
    } = $props();
    let panel: HTMLDivElement;
    let backButton: HTMLButtonElement;
    onMount(() => {
        if (!covered) backButton.focus({ preventScroll: true });
    });
    let wasCovered = untrack(() => covered);
    function focusIfActive() {
        if (!covered && panel.isConnected) backButton.focus({ preventScroll: true });
    }
    $effect(() => {
        if (wasCovered && !covered) void tick().then(focusIfActive);
        wasCovered = covered;
    });
    export function focusBack() {
        backButton.focus({ preventScroll: true });
    }
</script>

<div class="ui-overlay-layer" data-navigation-covered={covered} transition:pageSlide>
    <div
        class="ui-overlay"
        bind:this={panel}
        data-kind={kind}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        aria-busy={disabled}
        inert={covered}
        aria-hidden={covered}
        tabindex="-1"
        use:edgeBack={{ onback: onclose, beforeback, enabled: !disabled && !covered }}
        onkeydown={trapFocus}
        data-ui-no-swipe
    >
        <header class="ui-page-header ui-navigation-header">
            <button
                bind:this={backButton}
                type="button"
                class="ui-icon-button ui-pressable"
                aria-label={$tr('uiPreview.back')}
                {disabled}
                onclick={() => requestBack(panel)}
                ><span class="ui-press-visual"><ArrowLeft aria-hidden="true" /></span></button
            >
        </header>
        <div class="ui-overlay-body">
            <h1 class="ui-detail-title">{title}</h1>
            {@render children()}
        </div>
    </div>
</div>
