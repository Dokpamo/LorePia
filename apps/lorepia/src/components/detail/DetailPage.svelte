<script lang="ts">
    import { getContext } from 'svelte';
    import type { Snippet } from 'svelte';
    import { DETAIL_SCROLL_CONTEXT, type DetailScrollListener } from './detail-scroll';

    interface Props {
        ariaLabel: string;
        content: Snippet;
        actions?: Snippet;
        className?: string;
        scrollClassName?: string;
        onScroll?: (scrollTop: number) => void;
        resetKey?: string;
        hasActions?: boolean;
    }

    let {
        ariaLabel,
        content,
        actions,
        className = '',
        scrollClassName = '',
        onScroll,
        resetKey = '',
        hasActions = true,
    }: Props = $props();
    let scrollElement = $state<HTMLDivElement>();
    let previousResetKey = '';
    const inheritedOnScroll = getContext<DetailScrollListener | undefined>(DETAIL_SCROLL_CONTEXT);

    function notifyScroll(scrollTop: number): void {
        onScroll?.(scrollTop);
        inheritedOnScroll?.(scrollTop);
    }

    $effect(() => {
        if (resetKey === previousResetKey) return;
        previousResetKey = resetKey;
        queueMicrotask(() => {
            const scroller = scrollElement;
            if (!scroller) return;
            scroller.scrollTop = 0;
            notifyScroll(0);
        });
    });

    function handleScroll(event: Event): void {
        const scroller = event.currentTarget as HTMLDivElement;
        notifyScroll(scroller.scrollTop);
    }
</script>

<section class={`detail-page ${className}`.trim()} aria-label={ariaLabel}>
    <div
        bind:this={scrollElement}
        class={`detail-page-scroll ${scrollClassName}`.trim()}
        class:detail-page-has-actions={hasActions}
        onscroll={handleScroll}
    >
        {@render content()}
    </div>

    {@render actions?.()}
</section>

<style>
    .detail-page {
        position: relative;
        display: flex;
        height: 100%;
        min-height: 0;
        flex-direction: column;
    }

    .detail-page-scroll {
        display: grid;
        height: 0;
        min-height: 0;
        flex: 1 1 0;
        align-content: start;
        padding: 16px var(--settings-gutter) calc(24px + env(safe-area-inset-bottom));
        gap: 18px;
        overflow-y: auto;
    }

    .detail-page-scroll.detail-page-has-actions {
        padding-bottom: calc(var(--mobile-nav) + 36px + env(safe-area-inset-bottom));
    }

    :global(.app-shell[data-layout='desktop']) .detail-page-scroll {
        padding-top: 24px;
        padding-bottom: 40px;
        gap: 28px;
        scrollbar-gutter: auto;
    }

    :global(.app-shell[data-layout='desktop']) .detail-page-scroll.detail-page-has-actions {
        padding-bottom: calc(var(--mobile-nav) + 28px);
    }
</style>
