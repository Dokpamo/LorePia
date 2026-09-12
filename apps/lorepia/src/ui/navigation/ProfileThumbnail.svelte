<script lang="ts">
    import { onMount, setContext, untrack } from 'svelte';
    import { assetLoadPriorityContext } from '../../features/assets/asset-load-priority';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import CharacterImage from '../workspace/CharacterImage.svelte';
    import { releaseThumbnail, retainThumbnail } from './thumbnail-retention';
    import { observeThumbnail, thumbnailDistance } from './thumbnail-visibility';
    let { client, assetId, name }: { client: LorepiaClient; assetId: string; name: string } =
        $props();
    let element: HTMLSpanElement | undefined;
    setContext(assetLoadPriorityContext, () => (element ? thumbnailDistance(element) : Infinity));
    let mounted = $state(false);
    let visible = false;
    let decodedBytes = 0;
    let previousAssetId = untrack(() => assetId);
    let previousClient = untrack(() => client);
    $effect.pre(() => {
        const nextId = assetId;
        const nextClient = client;
        untrack(() => {
            if (nextId === previousAssetId && nextClient === previousClient) return;
            previousAssetId = nextId;
            previousClient = nextClient;
            if (element) releaseThumbnail(element);
            decodedBytes = 0;
            mounted = visible;
        });
    });
    onMount(() => {
        const thumbnail = element;
        if (!thumbnail) return;
        function decoded(event: Event) {
            const image = event.target;
            if (!(image instanceof HTMLImageElement) || !image.naturalWidth || !image.naturalHeight)
                return;
            const encodedBytes = Number(
                image.closest('[data-asset-bytes]')?.getAttribute('data-asset-bytes') ?? 0,
            );
            decodedBytes = image.naturalWidth * image.naturalHeight * 4 + encodedBytes;
        }
        thumbnail.addEventListener('load', decoded, true);
        const stopObserving = observeThumbnail(thumbnail, (near) => {
            visible = near;
            releaseThumbnail(thumbnail);
            if (near) mounted = true;
            else if (decodedBytes > 0) {
                retainThumbnail(thumbnail, decodedBytes, () => {
                    decodedBytes = 0;
                    mounted = false;
                });
            } else mounted = false;
        });
        return () => {
            stopObserving();
            thumbnail.removeEventListener('load', decoded, true);
            releaseThumbnail(thumbnail);
        };
    });
</script>

<span bind:this={element} class="seed-profile-thumbnail">
    {#if mounted}<CharacterImage {client} {assetId} {name} />{/if}
</span>

<style>
    .seed-profile-thumbnail {
        display: block;
        width: 100%;
        height: 100%;
        border-radius: inherit;
        background: transparent;
    }

    .seed-profile-thumbnail :global(.character-avatar-initial) {
        display: none;
    }

    .seed-profile-thumbnail
        :global([data-status-presentation='placeholder']:not([data-asset-phase='ready']) img) {
        visibility: visible;
    }

    .seed-profile-thumbnail :global([data-asset-phase='error'] .asset-error) {
        width: 100%;
        height: 100%;
        padding: 12px;
        clip-path: none;
        white-space: normal;
        color: var(--ui-muted);
        background: transparent;
        font-size: 12px;
        line-height: 1.5;
    }
</style>
