<script lang="ts">
    import type { Snippet } from 'svelte';
    import { ChevronRight } from '@lucide/svelte';
    let {
        label,
        value = '',
        disabled = false,
        onclick,
        prefix,
    }: {
        label: string;
        value?: string;
        disabled?: boolean;
        onclick: (event: MouseEvent & { currentTarget: HTMLButtonElement }) => void;
        prefix?: Snippet;
    } = $props();
</script>

<button type="button" class="ui-choice-field ui-pressable" {disabled} {onclick}>
    <span class="ui-press-visual">
        {#if prefix}<span class="ui-choice-prefix" aria-hidden="true">{@render prefix()}</span>{/if}
        <span>{label}</span>
        <span class="ui-choice-value">{value}</span>
        <ChevronRight aria-hidden="true" />
    </span>
</button>
