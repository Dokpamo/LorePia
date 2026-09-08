<script lang="ts">
    import type {
        AssetDeliverySelector,
        CharacterDto,
        LorepiaClient,
    } from '../../lib/ipc/contracts';
    import TrustedAsset from './TrustedAsset.svelte';

    const SHA256 = /^[0-9a-f]{64}$/;

    interface Props {
        client?: Partial<Pick<LorepiaClient, 'resolveAssetDelivery'>>;
        character?: Pick<CharacterDto, 'name' | 'avatar_asset_id'> | null;
        alt?: string;
    }

    let { client, character = null, alt }: Props = $props();
    const name = $derived(character?.name ?? '');
    const assetId = $derived(character?.avatar_asset_id ?? null);
    const assetAlt = $derived(alt ?? name);
    const initial = $derived(name.trim().slice(0, 1) || '?');
    const assetClient = $derived.by<Pick<LorepiaClient, 'resolveAssetDelivery'> | undefined>(() =>
        typeof client?.resolveAssetDelivery === 'function'
            ? { resolveAssetDelivery: client.resolveAssetDelivery.bind(client) }
            : undefined,
    );
    const selector = $derived.by<AssetDeliverySelector | null>(() => {
        if (assetId === null || assetId === '') return null;
        return SHA256.test(assetId)
            ? { kind: 'sha256', sha256: assetId }
            : { kind: 'asset_id', asset_id: assetId };
    });
</script>

<span class="character-avatar-initial" aria-hidden="true">{initial}</span>
{#if assetClient !== undefined && selector !== null}
    <TrustedAsset
        client={assetClient}
        {selector}
        expectedKind="image"
        alt={assetAlt}
        statusPresentation="placeholder"
    />
{/if}

<style>
    .character-avatar-initial {
        display: grid;
        width: 100%;
        height: 100%;
        place-items: center;
    }

    :global(.avatar > .trusted-asset) {
        position: absolute;
        inset: 0;
    }
</style>
