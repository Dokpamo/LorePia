<script lang="ts">
    interface SegmentOption {
        value: string;
        label: string;
        disabled?: boolean;
    }

    interface Props {
        id: string;
        label: string;
        value: string;
        options: SegmentOption[];
        onSelect: (value: string) => void;
        disabled?: boolean;
        className?: string;
    }

    let { id, label, value, options, onSelect, disabled = false, className = '' }: Props = $props();

    const selectedIndex = $derived(
        Math.max(
            0,
            options.findIndex((option) => option.value === value),
        ),
    );
    const segmentCount = $derived(Math.max(1, options.length));

    function enabledButtons(group: HTMLElement): HTMLButtonElement[] {
        return Array.from(group.querySelectorAll<HTMLButtonElement>('[role="radio"]')).filter(
            (button) => !button.disabled,
        );
    }

    function handleKeydown(event: KeyboardEvent): void {
        if (
            event.key !== 'ArrowLeft' &&
            event.key !== 'ArrowRight' &&
            event.key !== 'ArrowUp' &&
            event.key !== 'ArrowDown' &&
            event.key !== 'Home' &&
            event.key !== 'End'
        ) {
            return;
        }

        const button = event.currentTarget as HTMLButtonElement;
        const group = button.closest<HTMLElement>('[role="radiogroup"]');
        if (group === null) return;
        const buttons = enabledButtons(group);
        const currentIndex = buttons.indexOf(button);
        if (currentIndex < 0 || buttons.length === 0) return;

        event.preventDefault();
        const nextIndex =
            event.key === 'Home'
                ? 0
                : event.key === 'End'
                  ? buttons.length - 1
                  : event.key === 'ArrowLeft' || event.key === 'ArrowUp'
                    ? (currentIndex - 1 + buttons.length) % buttons.length
                    : (currentIndex + 1) % buttons.length;
        const nextButton = buttons[nextIndex];
        if (nextButton === undefined) return;
        nextButton.focus();
        const nextValue = nextButton.dataset.value;
        if (nextValue !== undefined) onSelect(nextValue);
    }
</script>

<div
    {id}
    class={`segmented-control ${className}`.trim()}
    role="radiogroup"
    aria-label={label}
    style={`--segment-count: ${String(segmentCount)}; --segment-index: ${String(selectedIndex)};`}
>
    <span class="segmented-control-thumb" aria-hidden="true"></span>
    {#each options as option (option.value)}
        <button
            type="button"
            role="radio"
            data-value={option.value}
            aria-checked={option.value === value}
            tabindex={option.value === value ? 0 : -1}
            disabled={disabled || option.disabled}
            onclick={() => onSelect(option.value)}
            onkeydown={handleKeydown}
        >
            <span>{option.label}</span>
        </button>
    {/each}
</div>

<style>
    .segmented-control {
        position: relative;
        display: grid;
        width: 100%;
        min-width: 0;
        min-height: 34px;
        padding: 2px;
        border-radius: 10px;
        background: var(--surface-active);
        grid-template-columns: repeat(var(--segment-count), minmax(0, 1fr));
        isolation: isolate;
    }

    .segmented-control-thumb {
        position: absolute;
        z-index: 0;
        top: 2px;
        bottom: 2px;
        left: 2px;
        width: calc((100% - 4px) / var(--segment-count));
        border-radius: 8px;
        background: var(--surface-raised);
        box-shadow: var(--shadow-1);
        pointer-events: none;
        transform: translate3d(calc(var(--segment-index) * 100%), 0, 0);
        transition: transform 220ms cubic-bezier(0.22, 1, 0.36, 1);
        will-change: transform;
    }

    .segmented-control button {
        position: relative;
        z-index: 1;
        width: 100%;
        min-width: 0;
        min-height: 30px;
        padding: 0 10px;
        border: 0;
        border-radius: 8px;
        background: transparent;
        box-shadow: none;
        color: var(--ink-muted);
        font-size: 0.75rem;
        font-weight: 600;
        transition: color 160ms ease;
    }

    .segmented-control button[aria-checked='true'] {
        background: transparent;
        color: var(--ink);
    }

    .segmented-control button:active:not(:disabled) {
        background: transparent;
    }

    .segmented-control button:focus-visible {
        outline: 2px solid var(--accent);
        outline-offset: -2px;
    }

    .segmented-control button:disabled {
        cursor: not-allowed;
        opacity: var(--disabled-opacity);
    }

    @media (prefers-reduced-motion: reduce) {
        .segmented-control-thumb {
            transition-duration: 0.01ms;
        }
    }
</style>
