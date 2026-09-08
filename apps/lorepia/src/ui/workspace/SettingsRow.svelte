<script lang="ts">
    import { ChevronRight } from '@lucide/svelte';
    let {
        label,
        value = '',
        disabled = false,
        onclick,
    }: {
        label: string;
        value?: string;
        disabled?: boolean;
        onclick: (event: MouseEvent & { currentTarget: HTMLButtonElement }) => void;
    } = $props();
</script>

<button type="button" class="ui-choice-field ui-pressable" {disabled} {onclick}>
    <span class="ui-press-visual">
        <span>{label}</span>
        <span class="ui-choice-value">{value}</span>
        <ChevronRight aria-hidden="true" />
    </span>
</button>
