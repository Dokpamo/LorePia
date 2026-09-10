<script lang="ts">
    import type { Snippet } from 'svelte';
    import { ChevronRight } from '@lucide/svelte';

    let {
        label,
        value,
        onopen,
        prefix,
        disabled = false,
        readonly = false,
    }: {
        label: string;
        value: string;
        onopen: (opener: HTMLButtonElement) => void;
        prefix?: Snippet;
        disabled?: boolean;
        readonly?: boolean;
    } = $props();
    const id = $props.id();
</script>

{#snippet content()}
    <span class="ui-press-visual">
        {#if prefix}<span class="ui-choice-prefix" aria-hidden="true">{@render prefix()}</span>{/if}
        <span class="ui-choice-label">{label}</span>
        <span class="ui-choice-value" {id}>{value}</span>
        {#if !readonly}<ChevronRight aria-hidden="true" />{/if}
    </span>
{/snippet}

{#if readonly}
    <div class="ui-choice-field" role="group" aria-label={label} aria-describedby={id}>
        {@render content()}
    </div>
{:else}
    <button
        type="button"
        class="ui-choice-field ui-pressable"
        aria-label={label}
        aria-describedby={id}
        aria-haspopup="dialog"
        {disabled}
        onclick={(event) => onopen(event.currentTarget)}
    >
        {@render content()}
    </button>
{/if}
