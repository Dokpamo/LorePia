<script lang="ts">
    import { ChevronRight } from '@lucide/svelte';
    import { useTextEditor } from '../../../ui/workspace/text-editor.svelte';
    import { tr } from '../../../lib/i18n';
    let {
        label,
        value,
        onchange,
        maxlength,
        rows = 1,
        disabled = false,
        required = false,
        placeholder = '',
    }: {
        label: string;
        value: string;
        onchange: (value: string) => void;
        maxlength: number;
        rows?: number;
        disabled?: boolean;
        required?: boolean;
        placeholder?: string;
    } = $props();
    const editor = useTextEditor();
</script>

{#if editor}
    <button
        type="button"
        class="ui-edit-field ui-pressable"
        aria-label={label}
        {disabled}
        onclick={(event) =>
            editor.open(
                {
                    label,
                    value,
                    maxlength,
                    placeholder,
                    applyOnDone: true,
                    requiredMessage: required ? $tr('settingsLive.requiredText') : undefined,
                    onchange,
                },
                event.currentTarget,
            )}
    >
        <span class="ui-edit-field-copy ui-press-visual"
            ><span>{label}</span><strong class:ui-field-empty={!value.trim()}
                >{value.trim() ? value : placeholder || $tr('uiPreview.tapToEdit')}</strong
            ></span
        ><ChevronRight aria-hidden="true" />
    </button>
{:else}
    <label
        >{label}{#if rows > 1}<textarea
                {rows}
                {value}
                {maxlength}
                {disabled}
                {required}
                {placeholder}
                oninput={(event) => onchange(event.currentTarget.value)}></textarea>{:else}<input
                {value}
                {maxlength}
                {disabled}
                {required}
                {placeholder}
                oninput={(event) => onchange(event.currentTarget.value)}
            />{/if}</label
    >
{/if}
