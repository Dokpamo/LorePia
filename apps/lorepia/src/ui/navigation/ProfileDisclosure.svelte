<script lang="ts">
    import { ChevronDown } from '@lucide/svelte';
    import { slide } from 'svelte/transition';
    import { cubicOut } from 'svelte/easing';
    import type { Snippet } from 'svelte';
    let {
        title,
        detail = '',
        children,
    }: { title: string; detail?: string; children: Snippet } = $props();
    const id = $props.id();
    let expanded = $state(false);
</script>

<section class="seed-profile-disclosure" data-expanded={expanded}>
    <h2>
        <button
            type="button"
            id={`${id}-trigger`}
            class="ui-pressable"
            aria-expanded={expanded}
            aria-controls={`${id}-content`}
            aria-labelledby={detail ? `${id}-title ${id}-detail` : `${id}-title`}
            onclick={() => (expanded = !expanded)}
        >
            <span class="ui-press-visual">
                <span class="seed-profile-disclosure-label"
                    ><strong id={`${id}-title`}>{title}</strong>{#if detail}<small
                            id={`${id}-detail`}>{detail}</small
                        >{/if}</span
                >
                <ChevronDown aria-hidden="true" />
            </span>
        </button>
    </h2>
    {#if expanded}
        <div
            id={`${id}-content`}
            role="region"
            aria-labelledby={`${id}-trigger`}
            transition:slide={{
                duration: matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 200,
                easing: cubicOut,
            }}
        >
            <div class="seed-profile-disclosure-content">{@render children()}</div>
        </div>
    {/if}
</section>
