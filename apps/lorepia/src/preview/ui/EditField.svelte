<script lang="ts">
    import { ChevronRight } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import { useTextEditor } from './text-editor.svelte';
    let {
        label,
        value,
        placeholder = '',
        maxlength,
        onchange,
    }: {
        label: string;
        value: string;
        placeholder?: string;
        maxlength: number;
        onchange: (value: string) => void;
    } = $props();
    const editor = useTextEditor();
</script>

<button
    type="button"
    class="ui-edit-field ui-pressable"
    aria-label={label}
    onclick={(event) =>
        editor.open({ label, value, placeholder, maxlength, onchange }, event.currentTarget)}
>
    <span class="ui-edit-field-copy ui-press-visual"
        ><span>{label}</span>
        <strong class:ui-field-empty={!value}
            >{value || placeholder || $tr('uiPreview.tapToEdit')}</strong
        ></span
    >
    <ChevronRight aria-hidden="true" />
</button>
