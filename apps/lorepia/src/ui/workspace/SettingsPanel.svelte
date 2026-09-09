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
        root = false,
        showTitle = true,
        footer,
        children,
    }: {
        title: string;
        kind?: string;
        onclose: () => void;
        beforeback?: () => boolean;
        disabled?: boolean;
        covered?: boolean;
        root?: boolean;
        showTitle?: boolean;
        footer?: Snippet;
        children: Snippet;
    } = $props();
    let panel: HTMLDivElement;
    let scrolled = $state(false);
    let backButton = $state<HTMLButtonElement>();
    onMount(() => {
        if (!covered && !root) backButton?.focus({ preventScroll: true });
    });
    let wasCovered = untrack(() => covered);
    function focusIfActive() {
        if (!covered && !root && panel.isConnected) backButton?.focus({ preventScroll: true });
    }
    $effect(() => {
        if (wasCovered && !covered) void tick().then(focusIfActive);
        wasCovered = covered;
    });
    export function focusBack() {
        backButton?.focus({ preventScroll: true });
    }
    function panelMotion(node: HTMLElement) {
        return root ? { duration: 0 } : pageSlide(node);
    }
</script>

<div
    class="ui-overlay-layer"
    data-root-panel={root}
    data-navigation-covered={covered}
    transition:panelMotion
>
    <div
        class="ui-overlay"
        bind:this={panel}
        data-kind={kind}
        data-scrolled={scrolled}
        role={root ? 'region' : 'dialog'}
        aria-modal={root ? undefined : true}
        aria-label={title}
        aria-busy={disabled}
        inert={covered}
        aria-hidden={covered}
        tabindex="-1"
        use:edgeBack={{ onback: onclose, beforeback, enabled: !root && !disabled && !covered }}
        onkeydown={root ? undefined : trapFocus}
        data-ui-no-swipe
    >
        <header class="ui-page-header ui-navigation-header" class:seed-header-root={root}>
            {#if root}<h1 class="seed-root-title">{title}</h1>{:else}
                <button
                    bind:this={backButton}
                    type="button"
                    class="ui-icon-button ui-pressable"
                    aria-label={$tr('uiPreview.back')}
                    {disabled}
                    onclick={() => requestBack(panel)}
                    ><span class="ui-press-visual"><ArrowLeft aria-hidden="true" /></span></button
                >
            {/if}
        </header>
        <div
            class="ui-overlay-body"
            onscroll={(event) => (scrolled = event.currentTarget.scrollTop > 8)}
        >
            {#if !root && showTitle}<h1 class="ui-detail-title">{title}</h1>{/if}
            {@render children()}
        </div>
        {#if footer}<div class="ui-panel-footer">{@render footer()}</div>{/if}
    </div>
</div>
