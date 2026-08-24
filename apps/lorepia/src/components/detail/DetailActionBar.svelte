<script lang="ts">
    import type { Snippet } from 'svelte';

    interface Props {
        ariaLabel: string;
        children: Snippet;
        className?: string;
        fixed?: boolean;
    }

    let { ariaLabel, children, className = '', fixed = false }: Props = $props();
</script>

<div
    class={`detail-action-bar ${className}`.trim()}
    class:fixed
    role="toolbar"
    aria-label={ariaLabel}
>
    {@render children()}
</div>

<style>
    .detail-action-bar {
        position: absolute;
        z-index: 20;
        right: auto;
        bottom: calc(8px + env(safe-area-inset-bottom));
        left: 50%;
        display: flex;
        width: min(calc(100% - var(--gutter) - var(--gutter)), 560px);
        height: var(--mobile-nav);
        min-height: var(--mobile-nav);
        padding: 0;
        border: 0;
        border-radius: 0;
        margin: 0;
        background: transparent;
        box-shadow: none;
        gap: clamp(4px, 1.144vw, 6px);
        transform: translateX(-50%);
    }

    .detail-action-bar :global(.detail-action) {
        display: inline-flex;
        height: 100%;
        min-width: 0;
        min-height: 0;
        flex: 1;
        align-items: center;
        justify-content: center;
        padding: 0 clamp(12px, 3.661vw, 16px);
        border-radius: var(--radius-pill);
        box-shadow: var(--shadow-2);
        font-size: var(--detail-support-type);
        font-weight: 700;
        gap: 8px;
    }

    .detail-action-bar :global(.detail-action--grow) {
        flex: 2;
    }

    .detail-action-bar :global(.detail-action--destructive) {
        color: #ff0000;
    }

    .detail-action-bar :global(.detail-action--borderless) {
        border: 0;
    }

    .detail-action-bar :global(.detail-action--wide) {
        flex: 1;
    }

    .detail-action-bar.fixed {
        position: fixed;
        left: var(--detail-action-center, 50%);
        width: min(
            calc(
                var(--detail-action-workspace-width, 100vw) -
                    var(--detail-action-fixed-gutter, var(--gutter)) -
                    var(--detail-action-fixed-gutter, var(--gutter))
            ),
            560px
        );
        transition:
            left 300ms cubic-bezier(0.22, 0.61, 0.36, 1),
            width 300ms cubic-bezier(0.22, 0.61, 0.36, 1);
    }

    @media (prefers-reduced-motion: reduce) {
        .detail-action-bar.fixed {
            transition: none;
        }
    }
</style>
