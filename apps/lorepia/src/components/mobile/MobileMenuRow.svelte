<script lang="ts">
    import { ChevronRight } from '@lucide/svelte';
    import type { Snippet } from 'svelte';

    interface Props {
        label: string;
        value?: string;
        description?: string;
        accent?: boolean;
        leading?: Snippet;
        onSelect: () => void;
    }

    let { label, value, description, accent = false, leading, onSelect }: Props = $props();
</script>

<button class="mobile-menu-row" type="button" onclick={onSelect}>
    {#if leading}<span class="mobile-menu-leading" aria-hidden="true">{@render leading()}</span
        >{/if}
    <span class="mobile-menu-copy">
        <span>{label}</span>
        {#if description}<small>{description}</small>{/if}
    </span>
    {#if value}<span class="mobile-menu-value" class:accent>{value}</span>{/if}
    <ChevronRight class="mobile-chevron" aria-hidden="true" />
</button>
