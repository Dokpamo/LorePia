<script lang="ts">
    import { ChevronRight } from '@lucide/svelte';
    import ChoiceField from '../../../components/ChoiceField.svelte';
    import { useOptionalChoiceSheet } from '../../../ui/workspace/choice-sheet.svelte';
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
    }: {
        id: string;
        label: string;
        value: string;
        options: { value: string; label: string; disabled?: boolean }[];
        onSelect: (value: string) => void;
        disabled?: boolean;
        required?: boolean;
        hint?: string;
        className?: string;
    } = $props();
    const sheet = useOptionalChoiceSheet();
</script>

{#if sheet}
    <button
        {id}
        type="button"
        class="ui-choice-field ui-pressable"
        aria-label={label}
        aria-describedby={id + '-value'}
        aria-haspopup="dialog"
        {disabled}
        onclick={(event) =>
            sheet.open(
                {
                    label,
                    value,
                    options: options.filter((item) => !item.disabled),
                    onselect: onSelect,
                },
                event.currentTarget,
            )}
    >
        <span class="ui-press-visual"
            ><span>{label}</span><span class="ui-choice-value" id={id + '-value'}
                >{options.find((item) => item.value === value)?.label ?? value}</span
            ><ChevronRight aria-hidden="true" /></span
        >
    </button>
    {#if hint}<p class="settings-note">{hint}</p>{/if}
{:else}
    <ChoiceField
        {id}
        {label}
        {value}
        {options}
        {onSelect}
        {disabled}
        {required}
        {hint}
        {className}
    />
{/if}
