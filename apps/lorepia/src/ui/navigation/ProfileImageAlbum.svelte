<script lang="ts">
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { ProfileImage } from './character-profile-types';
    import ProfileThumbnail from './ProfileThumbnail.svelte';
    let {
        client,
        images,
        onview,
    }: {
        client: LorepiaClient;
        images: ProfileImage[];
        onview: (image: ProfileImage, trigger: HTMLButtonElement) => void;
    } = $props();
</script>

<div class="seed-profile-photo-grid">
    {#each images as image (image.assetId)}
        <button
            type="button"
            class="ui-pressable"
            data-press-feedback="scale"
            aria-label={$tr('navigation.imageSelectItem', { name: image.title })}
            onclick={(event) => onview(image, event.currentTarget)}
        >
            <span class="ui-press-visual"
                ><ProfileThumbnail {client} assetId={image.assetId} name={image.title} /></span
            >
        </button>
    {/each}
</div>
