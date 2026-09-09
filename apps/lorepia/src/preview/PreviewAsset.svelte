<script lang="ts">
    import { previewImageSource } from './fixture-media';

    let {
        assetId,
        name,
        fit = 'cover',
    }: { assetId: string; name: string; fit?: 'cover' | 'contain' } = $props();
    const source = $derived(previewImageSource(assetId));
</script>

{#if source}
    <img src={source} alt={name} style:object-fit={fit} draggable="false" />
{:else}
    <span role="img" aria-label={name}>{name.slice(0, 1)}</span>
{/if}

<style>
    img,
    span {
        display: block;
        width: 100%;
        height: 100%;
    }

    img {
        object-position: 50% 30%;
    }

    span {
        display: grid;
        place-items: center;
        background: var(--ui-fill);
        color: var(--ui-ink);
    }
</style>
