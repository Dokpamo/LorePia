<script lang="ts">
    import type { Snippet } from 'svelte';
    let {
        onclick,
        disabled = false,
        children,
    }: { onclick: () => void; disabled?: boolean; children: Snippet } = $props();
</script>

<button type="button" class="ui-submit ui-pressable" {disabled} {onclick}
    ><span class="ui-press-visual">{@render children()}</span></button
>
