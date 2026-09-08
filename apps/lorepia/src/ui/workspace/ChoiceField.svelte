<script lang="ts">
    import { ChevronRight } from '@lucide/svelte';

    let {
        label,
        value,
        onopen,
    }: {
        label: string;
        value: string;
        onopen: (opener: HTMLButtonElement) => void;
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
        <span>{label}</span>
        <span class="ui-choice-value" {id}>{value}</span>
        <ChevronRight aria-hidden="true" />
    </span>
</button>
