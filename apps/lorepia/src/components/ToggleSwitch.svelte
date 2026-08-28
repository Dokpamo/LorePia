<script lang="ts">
    interface Props {
        label: string;
        checked: boolean;
        onChange: (checked: boolean) => void;
        disabled?: boolean;
        showLabel?: boolean;
        className?: string;
    }

    let {
        label,
        checked,
        onChange,
        disabled = false,
        showLabel = false,
        className = '',
    }: Props = $props();
</script>

<button
    type="button"
    class={`toggle-switch ${showLabel ? 'with-label' : ''} ${className}`.trim()}
    role="switch"
    aria-label={label}
    aria-checked={checked}
    {disabled}
    onclick={() => onChange(!checked)}
>
    {#if showLabel}<span class="toggle-switch-label">{label}</span>{/if}
    <span class="toggle-switch-track" aria-hidden="true">
        <span class="toggle-switch-thumb"></span>
    </span>
</button>

<style>
    .toggle-switch {
        display: inline-flex;
        width: 44px;
        height: 32px;
        min-height: 32px;
        flex: none;
        align-items: center;
        justify-content: center;
        padding: 6px 4px;
        border: 0;
        border-radius: 10px;
        background: transparent;
        box-shadow: none;
        color: var(--ink);
    }

    .toggle-switch.with-label {
        width: 100%;
        height: auto;
        min-height: 44px;
        justify-content: space-between;
        padding: 0 12px;
        text-align: left;
    }

    .toggle-switch-label {
        min-width: 0;
        overflow: hidden;
        font-size: 0.8125rem;
        font-weight: 600;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .toggle-switch-track {
        position: relative;
        width: 36px;
        height: 20px;
        flex: 0 0 36px;
        border-radius: var(--radius-pill);
        background: var(--line-strong);
        transition: background-color 180ms ease;
    }

    .toggle-switch-thumb {
        position: absolute;
        top: 2px;
        left: 2px;
        width: 16px;
        height: 16px;
        border-radius: 50%;
        background: var(--surface-raised);
        box-shadow: var(--shadow-1);
        transform: translate3d(0, 0, 0);
        transition: transform 200ms cubic-bezier(0.22, 1, 0.36, 1);
        will-change: transform;
    }

    .toggle-switch[aria-checked='true'] .toggle-switch-track {
        background: var(--primary-bg);
    }

    .toggle-switch[aria-checked='true'] .toggle-switch-thumb {
        transform: translate3d(16px, 0, 0);
    }

    .toggle-switch:active:not(:disabled) {
        background: transparent;
    }

    .toggle-switch:focus-visible {
        outline: none;
    }

    .toggle-switch:focus-visible .toggle-switch-track {
        box-shadow: 0 0 0 2px var(--accent);
    }

    .toggle-switch:disabled {
        cursor: not-allowed;
        opacity: var(--disabled-opacity);
    }

    @media (hover: hover) and (pointer: fine) {
        .toggle-switch:hover:not(:disabled) .toggle-switch-track {
            filter: brightness(0.97);
        }
    }

    @media (prefers-reduced-motion: reduce) {
        .toggle-switch-thumb {
            transition-duration: 0.01ms;
        }
    }
</style>
