<script lang="ts">
    import { ChevronRight } from '@lucide/svelte';

    let {
        label,
        onclick,
    }: {
        label: string;
        onclick: (trigger: HTMLButtonElement) => void;
    } = $props();
</script>

<button
    type="button"
    class="seed-profile-more ui-pressable"
    onclick={(event) => onclick(event.currentTarget)}
>
    <span class="ui-press-visual">
        <span>{label}</span>
        <ChevronRight aria-hidden="true" />
    </span>
</button>
