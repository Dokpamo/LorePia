<script lang="ts">
    let {
        label,
        checked,
        disabled = false,
        onchange,
    }: {
        label: string;
        checked: boolean;
        disabled?: boolean;
        onchange: (checked: boolean) => void;
    } = $props();
</script>

<button
    type="button"
    role="switch"
    aria-checked={checked}
    {disabled}
    onclick={() => onchange(!checked)}
>
    <span>{label}</span>
    <span class="track" aria-hidden="true"><span class="thumb"></span></span>
</button>

<style>
    button {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: var(--ui-space-4);
        width: 100%;
        min-height: var(--ui-row-compact);
        padding: var(--ui-space-3) 0;
        border: 0;
        background: transparent;
        color: var(--ui-ink);
        font: inherit;
        text-align: start;
    }
    .track {
        display: flex;
        flex-shrink: 0;
        align-items: center;
        width: 44px;
        height: 28px;
        padding: 3px;
        border-radius: 20px;
        background: var(--ui-selected);
        border: 1px solid var(--ui-muted);
        transition: background-color 180ms var(--ui-motion-ease);
    }
    .thumb {
        width: 20px;
        height: 20px;
        border-radius: 50%;
        background: var(--ui-muted);
        transition: transform 180ms var(--ui-motion-ease);
    }
    button[aria-checked='true'] .track {
        background: var(--ui-ink);
        border-color: var(--ui-ink);
    }
    button[aria-checked='true'] .thumb {
        transform: translateX(16px);
        background: var(--ui-paper);
    }
    button:focus-visible {
        outline: 2px solid var(--ui-ink);
        outline-offset: 4px;
        border-radius: var(--ui-radius);
    }
    button:disabled {
        opacity: 0.5;
    }
    @media (prefers-reduced-motion: reduce) {
        .track,
        .thumb {
            transition: none;
        }
    }
</style>
