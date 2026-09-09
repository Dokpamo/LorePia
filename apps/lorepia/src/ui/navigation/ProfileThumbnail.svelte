<script lang="ts">
    import { onMount } from 'svelte';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import CharacterImage from '../workspace/CharacterImage.svelte';
    let { client, assetId, name }: { client: LorepiaClient; assetId: string; name: string } =
        $props();
    let element: HTMLSpanElement;
    let visible = $state(false);
    onMount(() => {
        if (typeof IntersectionObserver === 'undefined') {
            visible = true;
            return;
        }
        const observer = new IntersectionObserver(
            (entries) => {
                if (entries.some((entry) => entry.isIntersecting)) {
                    visible = true;
                    observer.disconnect();
                }
            },
            { rootMargin: '160px' },
        );
        observer.observe(element);
        return () => observer.disconnect();
    });
</script>

<span bind:this={element} class="seed-profile-thumbnail">
    {#if visible}<CharacterImage {client} {assetId} {name} />{/if}
</span>

<style>
    .seed-profile-thumbnail {
        display: block;
        width: 100%;
        height: 100%;
        border-radius: inherit;
        background: var(--ui-soft);
    }
</style>
