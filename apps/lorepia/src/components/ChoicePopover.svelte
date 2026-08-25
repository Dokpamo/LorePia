<script lang="ts">
    import { Check, ChevronDown, ChevronUp } from '@lucide/svelte';
    import { cubicInOut, quintOut } from 'svelte/easing';
    import { tick } from 'svelte';
    import { fly } from 'svelte/transition';

    import { tr } from '../lib/i18n';

    interface ChoiceOption {
        value: string;
        label: string;
        disabled?: boolean;
    }

    interface Props {
        id: string;
        label: string;
        value: string;
        options: ChoiceOption[];
        onSelect: (value: string) => void;
        className?: string;
        disabled?: boolean;
        showLabel?: boolean;
    }

    let {
        id,
        label,
        value,
        options,
        onSelect,
        className = '',
        disabled = false,
        showLabel = true,
    }: Props = $props();
    let open = $state(false);
    let placement = $state<'above' | 'below'>('below');
    let rootElement = $state<HTMLDivElement | null>(null);
    let triggerElement = $state<HTMLButtonElement | null>(null);
    let menuElement = $state<HTMLDivElement | null>(null);
    let menuTop = $state(0);
    let menuLeft = $state(0);
    let menuWidth = $state(280);
    let menuMaxHeight = $state(344);
    let positionFrame: number | null = null;

    const selectedOption = $derived(options.find((option) => option.value === value) ?? null);
    const selectedLabel = $derived(selectedOption?.label ?? value);
    const menuId = $derived(`${id}-menu`);

    function enabledOptionButtons(): HTMLButtonElement[] {
        return Array.from(
            menuElement?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]') ?? [],
        ).filter((button) => !button.disabled);
    }

    function positionMenu(): void {
        if (triggerElement === null || typeof window === 'undefined') return;
        const triggerRect = triggerElement.getBoundingClientRect();
        const viewportWidth = document.documentElement.clientWidth || window.innerWidth;
        const viewportHeight = document.documentElement.clientHeight || window.innerHeight;
        const edge = 12;
        const gap = 6;
        const desiredWidth = Math.max(180, Math.min(310, viewportWidth - edge * 2));
        const desiredHeight = Math.min(
            Math.max(menuElement?.scrollHeight ?? 0, options.length * 54 + 20),
            344,
        );
        const spaceBelow = Math.max(0, viewportHeight - edge - triggerRect.bottom - gap);
        const spaceAbove = Math.max(0, triggerRect.top - edge - gap);
        placement = spaceBelow >= desiredHeight || spaceBelow >= spaceAbove ? 'below' : 'above';
        const availableHeight = placement === 'below' ? spaceBelow : spaceAbove;
        const nextMaxHeight = Math.max(48, Math.min(344, availableHeight));
        const renderedHeight = Math.min(desiredHeight, nextMaxHeight);
        const preferredTop =
            placement === 'below'
                ? triggerRect.bottom + gap
                : triggerRect.top - gap - renderedHeight;
        const maximumTop = Math.max(edge, viewportHeight - edge - renderedHeight);

        menuWidth = desiredWidth;
        menuMaxHeight = nextMaxHeight;
        menuLeft = Math.round(
            Math.min(
                viewportWidth - edge - desiredWidth,
                Math.max(edge, triggerRect.right - desiredWidth),
            ),
        );
        menuTop = Math.round(Math.min(maximumTop, Math.max(edge, preferredTop)));
    }

    function schedulePosition(): void {
        if (!open || typeof window === 'undefined') return;
        if (positionFrame !== null) window.cancelAnimationFrame(positionFrame);
        positionFrame = window.requestAnimationFrame(() => {
            positionFrame = null;
            positionMenu();
        });
    }

    function mountPopover(node: HTMLDivElement): { destroy: () => void } {
        menuElement = node;
        let topLayerActive = false;
        if (typeof node.showPopover === 'function') {
            try {
                node.showPopover();
                topLayerActive = node.matches(':popover-open');
            } catch {
                // The fallback remains a fixed element when the Popover API is unavailable.
            }
        }
        if (!topLayerActive) node.removeAttribute('popover');
        schedulePosition();
        return {
            destroy: () => {
                if (positionFrame !== null) window.cancelAnimationFrame(positionFrame);
                positionFrame = null;
                if (topLayerActive && typeof node.hidePopover === 'function') {
                    try {
                        node.hidePopover();
                    } catch {
                        // It may already have left the top layer during teardown.
                    }
                }
                if (menuElement === node) menuElement = null;
            },
        };
    }

    async function focusChoice(index: number): Promise<void> {
        await tick();
        const buttons = enabledOptionButtons();
        if (buttons.length === 0) return;
        buttons[Math.max(0, Math.min(index, buttons.length - 1))]?.focus();
    }

    async function setOpen(next: boolean, restoreFocus = false): Promise<void> {
        if (disabled) return;
        if (next) positionMenu();
        open = next;
        if (next) {
            await tick();
            positionMenu();
            const enabledOptions = options.filter((option) => !option.disabled);
            const selectedIndex = Math.max(
                0,
                enabledOptions.findIndex((option) => option.value === value),
            );
            await focusChoice(selectedIndex);
        } else if (restoreFocus) {
            await tick();
            triggerElement?.focus();
        }
    }

    function choose(option: ChoiceOption): void {
        if (option.disabled) return;
        onSelect(option.value);
        void setOpen(false, true);
    }

    function handleWindowPointerDown(event: PointerEvent): void {
        if (!open || rootElement === null) return;
        if (!event.composedPath().includes(rootElement)) void setOpen(false);
    }

    function handleWindowKeydown(event: KeyboardEvent): void {
        if (!open || event.key !== 'Escape') return;
        event.preventDefault();
        event.stopImmediatePropagation();
        void setOpen(false, true);
    }

    function handleTriggerKeydown(event: KeyboardEvent): void {
        if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
        event.preventDefault();
        void setOpen(true).then(() => {
            const buttons = enabledOptionButtons();
            if (event.key === 'ArrowUp') buttons.at(-1)?.focus();
        });
    }

    function handleOptionKeydown(event: KeyboardEvent): void {
        const buttons = enabledOptionButtons();
        const index = buttons.indexOf(event.currentTarget as HTMLButtonElement);
        if (event.key === 'ArrowDown') {
            event.preventDefault();
            buttons[(index + 1) % buttons.length]?.focus();
        } else if (event.key === 'ArrowUp') {
            event.preventDefault();
            buttons[(index - 1 + buttons.length) % buttons.length]?.focus();
        } else if (event.key === 'Home') {
            event.preventDefault();
            buttons[0]?.focus();
        } else if (event.key === 'End') {
            event.preventDefault();
            buttons.at(-1)?.focus();
        }
    }
</script>

<svelte:window
    onpointerdown={handleWindowPointerDown}
    onkeydowncapture={handleWindowKeydown}
    onresize={schedulePosition}
    onscrollcapture={schedulePosition}
/>

<div bind:this={rootElement} class={`choice-popover ${className}`.trim()}>
    <button
        bind:this={triggerElement}
        class:compact={!showLabel}
        class="choice-trigger"
        type="button"
        {disabled}
        aria-label={`${label}: ${selectedLabel}`}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={menuId}
        onclick={() => void setOpen(!open)}
        onkeydown={handleTriggerKeydown}
    >
        <span class="choice-copy">
            {#if showLabel}<strong>{label}</strong>{/if}
            <span class="choice-value">{selectedLabel}</span>
        </span>
        {#if open}
            <ChevronUp class="choice-chevron" aria-hidden="true" />
        {:else}
            <ChevronDown class="choice-chevron" aria-hidden="true" />
        {/if}
    </button>

    {#if open}
        <div
            id={menuId}
            class:above={placement === 'above'}
            class="choice-menu"
            popover="manual"
            use:mountPopover
            role="menu"
            aria-label={`${label} ${$tr('choice.menu.label')}`}
            style:top={`${String(menuTop)}px`}
            style:left={`${String(menuLeft)}px`}
            style:width={`${String(menuWidth)}px`}
            style:max-height={`${String(menuMaxHeight)}px`}
            in:fly={{
                y: placement === 'below' ? -10 : 10,
                duration: 240,
                easing: quintOut,
            }}
            out:fly={{
                y: placement === 'below' ? -5 : 5,
                duration: 170,
                easing: cubicInOut,
            }}
        >
            {#each options as option (option.value)}
                <button
                    type="button"
                    class="choice-option"
                    class:selected={option.value === value}
                    disabled={option.disabled}
                    role="menuitemradio"
                    aria-checked={option.value === value}
                    onclick={() => choose(option)}
                    onkeydown={handleOptionKeydown}
                >
                    <span>{option.label}</span>
                    {#if option.value === value}
                        <Check class="choice-check" aria-hidden="true" />
                    {/if}
                </button>
            {/each}
        </div>
    {/if}
</div>

<style>
    .choice-popover {
        position: relative;
        width: 100%;
        min-width: 0;
    }

    .choice-trigger {
        display: flex;
        width: 100%;
        min-width: 0;
        min-height: 64px;
        align-items: center;
        justify-content: space-between;
        padding: 10px 14px;
        border: 0;
        border-radius: 4px;
        background: transparent;
        box-shadow: none;
        color: var(--ink);
        gap: 12px;
        text-align: left;
    }

    .choice-trigger.compact {
        min-height: 38px;
        padding: 0 12px;
        border: 1px solid var(--line);
        border-radius: var(--radius-pill);
        background: var(--surface-raised);
    }

    .choice-copy {
        display: grid;
        min-width: 0;
        flex: 1;
        gap: 3px;
    }

    .choice-copy strong {
        overflow: hidden;
        font-size: 0.875rem;
        font-weight: 650;
        line-height: 1.2;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .choice-value {
        overflow: hidden;
        color: var(--ink-muted);
        font-size: 0.8125rem;
        font-weight: 500;
        line-height: 1.3;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .compact .choice-copy {
        display: block;
    }

    .compact .choice-value {
        color: var(--ink);
        font-size: 0.75rem;
    }

    .choice-trigger :global(.choice-chevron) {
        width: 20px;
        height: 20px;
        flex: none;
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 2;
    }

    .choice-menu {
        position: fixed;
        z-index: 80;
        inset: auto;
        display: grid;
        padding: 10px;
        border: 0;
        border-radius: 24px;
        margin: 0;
        background: var(--surface-sunken);
        box-shadow: var(--shadow-3);
        color: var(--ink);
        gap: 0;
        overflow-y: auto;
        overscroll-behavior: contain;
        transform-origin: top right;
    }

    .choice-menu.above {
        transform-origin: bottom right;
    }

    .choice-menu::backdrop {
        background: transparent;
        backdrop-filter: none;
    }

    .choice-option {
        display: flex;
        width: 100%;
        min-height: 54px;
        align-items: center;
        justify-content: space-between;
        padding: 0 16px;
        border: 0;
        border-radius: 14px;
        background: transparent;
        box-shadow: none;
        color: var(--ink);
        font-size: 0.9375rem;
        font-weight: 550;
        gap: 16px;
        text-align: left;
    }

    .choice-option.selected {
        font-weight: 700;
    }

    .choice-option :global(.choice-check) {
        width: 21px;
        height: 21px;
        flex: none;
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 2.3;
    }

    .choice-option:disabled {
        opacity: 0.42;
    }

    @media (hover: hover) and (pointer: fine) {
        .choice-trigger:hover:not(:disabled),
        .choice-option:hover:not(:disabled) {
            background: var(--surface-hover);
        }
    }

    @media (prefers-reduced-motion: reduce) {
        .choice-menu {
            scroll-behavior: auto;
        }
    }
</style>
