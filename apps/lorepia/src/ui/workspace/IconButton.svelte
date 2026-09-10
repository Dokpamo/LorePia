<script lang="ts">
    import type { Snippet } from 'svelte';

    let {
        label,
        onclick,
        disabled = false,
        children,
        caption,
        critical = false,
    }: {
        label: string;
        onclick: (event: MouseEvent & { currentTarget: HTMLButtonElement }) => void;
        disabled?: boolean;
        children: Snippet;
        caption?: string;
        critical?: boolean;
    } = $props();
</script>

<button
    type="button"
    class="ui-icon-button ui-pressable"
    class:ui-tool-button={!!caption}
    class:ui-critical={critical}
    aria-label={label}
    data-ui-tooltip={label}
    {onclick}
    {disabled}
>
    <span class="ui-press-visual" aria-hidden="true"
        >{@render children()}{#if caption}<span class="ui-tool-caption">{caption}</span>{/if}</span
    >
</button>
