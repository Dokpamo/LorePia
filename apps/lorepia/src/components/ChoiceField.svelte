<script lang="ts">
    import ChoicePopover from './ChoicePopover.svelte';

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
        disabled?: boolean;
        required?: boolean;
        hint?: string;
        className?: string;
    }

    let {
        id,
        label,
        value,
        options,
        onSelect,
        disabled = false,
        required = false,
        hint,
        className = '',
    }: Props = $props();
</script>

<div class={`choice-field ${className}`.trim()}>
    <label class="choice-field-label" for={`${id}-trigger`}>{label}</label>
    <ChoicePopover
        {id}
        {label}
        {value}
        {options}
        {onSelect}
        {disabled}
        {required}
        showLabel={false}
        variant="field"
    />
    {#if hint}<small class="choice-field-hint">{hint}</small>{/if}
</div>

<style>
    .choice-field {
        display: grid;
        min-width: 0;
        gap: 7px;
        color: var(--ink-muted);
        font-size: var(--detail-support-type, 0.8125rem);
        font-weight: 700;
    }

    .choice-field-label {
        color: inherit;
        font: inherit;
    }

    .choice-field-hint {
        color: var(--ink-muted);
        font-size: 0.8em;
        font-weight: 500;
        line-height: 1.45;
    }
</style>
