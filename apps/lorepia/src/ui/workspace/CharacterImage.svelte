<script lang="ts">
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import CharacterAvatar from '../../features/assets/CharacterAvatar.svelte';

    let {
        client,
        name,
        assetId,
    }: {
        client: Pick<LorepiaClient, 'resolveAssetDelivery'>;
        name: string;
        assetId: string;
    } = $props();
</script>

<span class="ui-character-image">
    <CharacterAvatar {client} character={{ name, avatar_asset_id: assetId }} alt={name} />
</span>

<style>
    .ui-character-image {
        position: relative;
        display: block;
        width: 100%;
        height: 100%;
        overflow: hidden;
        border-radius: inherit;
    }
    .ui-character-image :global(.trusted-asset) {
        position: absolute;
        inset: 0;
    }
</style>
