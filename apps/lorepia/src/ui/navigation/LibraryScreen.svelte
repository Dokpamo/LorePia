<script lang="ts">
    import { tick, type Snippet } from 'svelte';
    import { ArrowLeft, Search } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import IconButton from '../workspace/IconButton.svelte';
    import { edgeBack, requestBack } from '../workspace/edge-back';
    import { trapFocus } from '../workspace/focus-trap';
    import { pageSlide } from '../workspace/navigation-motion';
    import NavigationHeader from './NavigationHeader.svelte';
    import LibrarySearch from './LibrarySearch.svelte';

    let {
        title,
        searchLabel,
        regionLabel,
        query = $bindable(''),
        ondetail,
        actions: toolbarActions,
        children,
        results,
    }: {
        title: string;
        searchLabel: string;
        regionLabel?: string;
        query?: string;
        ondetail: (active: boolean) => void;
        actions: Snippet;
        children: Snippet;
        results: Snippet;
    } = $props();
    let searching = $state(false);
    let panel = $state<HTMLDivElement>();
    let searchField = $state<{ focusInput: () => void }>();
    let trigger: HTMLButtonElement | undefined;
    $effect(() => ondetail(searching));
    function closeSearch() {
        searching = false;
        void tick().then(() => trigger?.isConnected && trigger.focus({ preventScroll: true }));
    }
</script>

<div class="seed-library-frame ui-management">
    <div class="seed-library-base ui-management-content" inert={searching} aria-hidden={searching}>
        <NavigationHeader {title}>
            {#snippet actions()}
                <IconButton
                    label={searchLabel}
                    onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) => {
                        trigger = event.currentTarget;
                        searching = true;
                    }}><Search /></IconButton
                >
                {@render toolbarActions()}
            {/snippet}
        </NavigationHeader>
        <div class="seed-scroll" role={regionLabel ? 'region' : undefined} aria-label={regionLabel}>
            {@render children()}
        </div>
    </div>
    {#if searching}
        <div
            class="ui-overlay-layer"
            transition:pageSlide
            onintroend={() => {
                // WKWebView can scroll ancestors to an input while it is still offscreen.
                if (searching && panel && !panel.closest('[inert], [aria-hidden="true"]'))
                    searchField?.focusInput();
            }}
        >
            <div
                class="ui-overlay seed-search-page"
                bind:this={panel}
                role="dialog"
                aria-modal="true"
                aria-label={searchLabel}
                tabindex="-1"
                use:edgeBack={{ onback: closeSearch }}
                onkeydown={trapFocus}
                data-ui-no-swipe
            >
                <header class="seed-header seed-search-header">
                    <IconButton
                        label={$tr('uiPreview.back')}
                        onclick={() => panel && requestBack(panel)}><ArrowLeft /></IconButton
                    >
                    <LibrarySearch bind:this={searchField} bind:query label={searchLabel} />
                </header>
                <div class="seed-scroll">
                    <div class="seed-content seed-search-results">{@render results()}</div>
                </div>
            </div>
        </div>
    {/if}
</div>
