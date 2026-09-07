<script lang="ts">
    import type { Snippet } from 'svelte';

    let {
        label,
        onclick,
        disabled = false,
        children,
    }: {
        label: string;
        onclick: (event: MouseEvent & { currentTarget: HTMLButtonElement }) => void;
        disabled?: boolean;
        children: Snippet;
    } = $props();
</script>

<button type="button" class="ui-icon-button ui-pressable" aria-label={label} {onclick} {disabled}>
    <span class="ui-press-visual">{@render children()}</span>
</button>
