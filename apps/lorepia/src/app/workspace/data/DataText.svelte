<script lang="ts">
    import EditField from '../../../ui/workspace/EditField.svelte';
    let {
        label,
        value,
        maxlength = 32000,
        onchange,
        disabled = false,
        placeholder = '',
    }: {
        label: string;
        value: string;
        maxlength?: number;
        onchange: (value: string) => void;
        disabled?: boolean;
        placeholder?: string;
    } = $props();
</script>

<div inert={disabled}><EditField {label} {value} {maxlength} {onchange} {placeholder} /></div>
