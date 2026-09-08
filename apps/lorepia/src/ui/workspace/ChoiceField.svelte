<script lang="ts">
    import type { Snippet } from 'svelte';
    import { ChevronRight } from '@lucide/svelte';

    let {
        label,
        value,
        onopen,
        prefix,
    }: {
        label: string;
        value: string;
        onopen: (opener: HTMLButtonElement) => void;
        prefix?: Snippet;
    } = $props();
    const id = $props.id();
</script>

<button
    type="button"
    class="ui-choice-field ui-pressable"
    aria-label={label}
    aria-describedby={id}
    aria-haspopup="dialog"
    onclick={(event) => onopen(event.currentTarget)}
>
    <span class="ui-press-visual">
        {#if prefix}<span class="ui-choice-prefix" aria-hidden="true">{@render prefix()}</span>{/if}
        <span>{label}</span>
        <span class="ui-choice-value" {id}>{value}</span>
        <ChevronRight aria-hidden="true" />
    </span>
</button>
