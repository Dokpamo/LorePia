<script lang="ts">
    import { ArrowLeft, X } from '@lucide/svelte';
    import { fade } from 'svelte/transition';
    import { onMount, tick, untrack, type Snippet } from 'svelte';
    import { tr } from '../../lib/i18n';
    import { edgeBack, requestBack, type BackDecision } from './edge-back';
    import { trapFocus } from './focus-trap';
    import { pageSlide } from './navigation-motion';
    import SheetHandle from './SheetHandle.svelte';
    import { choiceSheetTransition } from './choice-sheet-motion';
    import { settingsSheetBack } from './settings-sheet-back';

    let {
        title,
        kind = 'app-settings',
        onclose,
        beforeback,
        disabled = false,
        covered = false,
        root = false,
        sheet = false,
        showTitle = true,
        inlineTitle = false,
        collapsedTitle,
        footer,
        children,
    }: {
        title: string;
        kind?: string;
        onclose: () => void;
        beforeback?: () => BackDecision;
        disabled?: boolean;
        covered?: boolean;
        root?: boolean;
        sheet?: boolean;
        showTitle?: boolean;
        inlineTitle?: boolean;
        collapsedTitle?: string;
        footer?: Snippet;
        children: Snippet;
    } = $props();
    let panel: HTMLDivElement;
    let backdrop = $state<HTMLButtonElement>();
    let expanded = $state(false);
    const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
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
        return root || sheet ? { duration: 0 } : pageSlide(node);
    }
    function sheetMotion(node: HTMLElement) {
        return sheet ? choiceSheetTransition(node, reduced) : { duration: 0 };
    }
</script>

<div
    class="ui-overlay-layer"
    class:ui-choice-layer={sheet}
    data-root-panel={root}
    data-navigation-covered={covered}
    transition:panelMotion
>
    {#if sheet}<button
            class="ui-choice-backdrop"
            bind:this={backdrop}
            aria-hidden="true"
            tabindex="-1"
            transition:fade={{ duration: reduced ? 0 : 300 }}
            onclick={() => requestBack(panel)}
        ></button>{/if}
    <div
        class="ui-overlay"
        class:ui-choice-sheet={sheet}
        data-sheet-expanded={sheet ? expanded : undefined}
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
        use:edgeBack={{
            onback: onclose,
            beforeback,
            enabled: !sheet && !root && !disabled && !covered,
        }}
        use:settingsSheetBack={{
            enabled: sheet && !disabled && !covered,
            backdrop: () => backdrop,
            beforeback,
            onclose,
        }}
        transition:sheetMotion
        onkeydown={root ? undefined : trapFocus}
        data-ui-no-swipe={root ? undefined : ''}
    >
        {#if sheet}<SheetHandle
                panel={() => panel}
                onclose={() => requestBack(panel)}
                bind:expanded
                disabled={disabled || covered}
            />{/if}
        <header
            class={sheet ? 'ui-sheet-header' : 'ui-page-header ui-navigation-header'}
            class:seed-header-root={root}
        >
            {#if sheet}<h2>{title}</h2>
                <button
                    bind:this={backButton}
                    type="button"
                    class="ui-icon-button ui-pressable"
                    aria-label={$tr('uiPreview.closeChoices')}
                    {disabled}
                    onclick={() => requestBack(panel)}
                    ><span class="ui-press-visual"><X aria-hidden="true" /></span></button
                >
            {:else}
                {#if root}<h1 class="seed-root-title">{title}</h1>{:else}
                    <button
                        bind:this={backButton}
                        type="button"
                        class="ui-icon-button ui-pressable"
                        aria-label={$tr('uiPreview.back')}
                        {disabled}
                        onclick={() => requestBack(panel)}
                        ><span class="ui-press-visual"><ArrowLeft aria-hidden="true" /></span
                        ></button
                    >
                    {#if inlineTitle && showTitle}<h1 class="ui-navigation-title">{title}</h1>{/if}
                    {#if collapsedTitle}<span class="ui-collapsed-title" aria-hidden="true"
                            >{collapsedTitle}</span
                        >{/if}
                {/if}
            {/if}
        </header>
        <div
            class="ui-overlay-body"
            onscroll={(event) => (scrolled = event.currentTarget.scrollTop > 8)}
        >
            {#if !sheet && !root && showTitle && !inlineTitle}<h1 class="ui-detail-title">
                    {title}
                </h1>{/if}
            {@render children()}
        </div>
        {#if footer}<div class="ui-panel-footer">{@render footer()}</div>{/if}
    </div>
</div>

<style>
    .ui-overlay.ui-choice-sheet {
        position: relative;
        inset: auto;
        padding-inline: 16px;
        background: var(--ui-paper);
    }
    .ui-choice-sheet .ui-overlay-body {
        padding: 8px 0 max(24px, env(safe-area-inset-bottom));
        overscroll-behavior: contain;
    }
    .ui-sheet-header h2 {
        flex: 1;
    }
    .ui-navigation-title {
        margin: 0;
        padding-inline: var(--ui-space-1);
        color: var(--ui-heading);
        font-size: var(--ui-type-nav);
        line-height: 1.333333;
        font-weight: 700;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
</style>
