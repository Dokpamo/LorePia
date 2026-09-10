<script lang="ts">
    import ChoiceField from '../../../ui/workspace/ChoiceField.svelte';
    import { useChoiceSheet } from '../../../ui/workspace/choice-sheet.svelte';
    let {
        label,
        value,
        options,
        onSelect,
        disabled = false,
    }: {
        label: string;
        value: string;
        options: { value: string; label: string }[];
        onSelect: (value: string) => void;
        disabled?: boolean;
    } = $props();
    const sheet = useChoiceSheet();
</script>

<div inert={disabled}>
    <ChoiceField
        {label}
        value={options.find((item) => item.value === value)?.label ?? value}
        onopen={(opener: HTMLButtonElement) =>
            sheet.open({ label, value, options, onselect: onSelect }, opener)}
    />
</div>
