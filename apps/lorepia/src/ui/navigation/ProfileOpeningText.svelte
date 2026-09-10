<script lang="ts">
    import { getContext } from 'svelte';
    import MarkdownText from '../../features/chat/MarkdownText.svelte';
    import TrustedAsset from '../../features/assets/TrustedAsset.svelte';
    import type { CharacterRenderAssetDto, LorepiaClient } from '../../lib/ipc/contracts';
    import {
        assetPresentationContext,
        type AssetPresentation,
    } from '../workspace/asset-presentation';
    import { openingParts } from './profile-opening-text';
    let {
        text,
        assets,
        client,
    }: { text: string; assets: CharacterRenderAssetDto[]; client: LorepiaClient } = $props();
    const AssetView = getContext<AssetPresentation | undefined>(assetPresentationContext);
    const parts = $derived(openingParts(text, assets));
</script>

{#each parts as part, index (index)}
    {#if part.kind === 'text'}<MarkdownText text={part.text} />
    {:else}
        <figure class="seed-opening-image">
            {#if AssetView}<AssetView
                    assetId={part.assetId}
                    name={part.name}
                    fit="contain"
                />{:else}
                <TrustedAsset
                    {client}
                    selector={part.selector}
                    alt={part.name}
                    expectedKind="image"
                />
            {/if}
        </figure>
    {/if}
{/each}
