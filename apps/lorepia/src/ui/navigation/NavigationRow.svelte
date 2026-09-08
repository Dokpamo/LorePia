<script lang="ts">
    import type { Snippet } from 'svelte';
    import { ChevronRight } from '@lucide/svelte';
    let {
        title,
        description = '',
        onclick,
        prefix,
        disabled = false,
    }: {
        title: string;
        description?: string;
        onclick: (event: MouseEvent & { currentTarget: HTMLButtonElement }) => void;
        prefix?: Snippet;
        disabled?: boolean;
    } = $props();
</script>

<button class="seed-row ui-pressable" {onclick} {disabled}>
    <span class="ui-press-visual">
        {#if prefix}<span class="seed-row-prefix" aria-hidden="true">{@render prefix()}</span>{/if}
        <span class="seed-row-copy"
            ><strong>{title}</strong>{#if description}<small>{description}</small>{/if}</span
        >
        <ChevronRight class="seed-row-chevron" aria-hidden="true" />
    </span>
</button>
