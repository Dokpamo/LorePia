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
        hint,
        requiredMessage,
        error,
    }: {
        label: string;
        value: string;
        placeholder?: string;
        maxlength: number;
        onchange: (value: string) => void;
        hint?: string;
        requiredMessage?: string;
        error?: string;
    } = $props();
    const editor = useTextEditor();
    const ids = $props.id();
</script>

<div class="ui-edit-field-group">
    <button
        type="button"
        class="ui-edit-field ui-pressable"
        aria-label={label}
        data-invalid={!!error}
        aria-describedby={[requiredMessage ? ids + '-required' : '', error ? ids + '-error' : '']
            .filter(Boolean)
            .join(' ') || undefined}
        onclick={(event) =>
            editor.open(
                { label, value, placeholder, maxlength, onchange, hint, requiredMessage },
                event.currentTarget,
            )}
    >
        <span class="ui-press-visual"
            ><span class="ui-edit-field-copy"
                ><span
                    >{label}{#if requiredMessage}<span
                            id={ids + '-required'}
                            class="ui-field-requirement">{$tr('uiPreview.required')}</span
                        >{/if}</span
                >
                <strong class:ui-field-empty={!value.trim()}
                    >{value.trim() ? value : placeholder || $tr('uiPreview.tapToEdit')}</strong
                ></span
            >
            <ChevronRight aria-hidden="true" /></span
        >
    </button>
    {#if error}<p id={ids + '-error'} class="ui-field-error" role="alert">{error}</p>{/if}
</div>
