<script lang="ts">
    import ChoiceField from '../../../ui/workspace/ChoiceField.svelte';
    import { useChoiceSheet } from '../../../ui/workspace/choice-sheet.svelte';
    let {
        label,
        value,
        options,
        onselect,
    }: {
        label: string;
        value: string;
        options: { value: string; label: string }[];
        onselect: (value: string) => void;
    } = $props();
    const choice = useChoiceSheet();
</script>

<ChoiceField
    {label}
    value={options.find((o) => o.value === value)?.label ?? value}
    onopen={(opener: HTMLButtonElement) => choice.open({ label, value, options, onselect }, opener)}
/>
