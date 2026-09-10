<script lang="ts">
    import EditField from '../../../ui/workspace/EditField.svelte';
    import { tr } from '../../../lib/i18n';
    let {
        label,
        value,
        onchange,
        min = 0,
        max = 2000000,
        disabled = false,
        integer = true,
    }: {
        label: string;
        value: number | null;
        onchange: (value: number) => void;
        min?: number;
        max?: number;
        disabled?: boolean;
        integer?: boolean;
    } = $props();
    let invalid = $state(false);
    function change(text: string) {
        const next = Number(text);
        invalid =
            !text.trim() ||
            !Number.isFinite(next) ||
            (integer && !Number.isSafeInteger(next)) ||
            next < min ||
            next > max;
        if (!invalid) onchange(next);
    }
</script>

<div inert={disabled}>
    <EditField
        {label}
        value={String(value ?? '')}
        maxlength={20}
        onchange={change}
        error={invalid ? $tr('settingsUi.invalidNumber') : undefined}
    />
</div>
