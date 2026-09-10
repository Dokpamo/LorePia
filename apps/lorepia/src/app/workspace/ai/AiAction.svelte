<script lang="ts">
    let {
        label,
        onclick,
        disabled = false,
        secondary = false,
    }: { label: string; onclick: () => void; disabled?: boolean; secondary?: boolean } = $props();
</script>

<button type="button" class="ui-submit ui-pressable" class:secondary {disabled} {onclick}
    ><span class="ui-press-visual">{label}</span></button
>

<style>
    .secondary {
        --ui-action: var(--ui-soft);
        --ui-action-pressed: var(--ui-pressed-surface);
        --ui-on-action: var(--ui-ink);
    }
</style>
