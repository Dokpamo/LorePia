<script lang="ts">
    import { tick, untrack } from 'svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type {
        ProfileImage,
        ProfileImageGroup,
        ProfileResourceDetail,
    } from './character-profile-types';
    import SettingsPanel from '../workspace/SettingsPanel.svelte';
    import ProfileImageGroups from './ProfileImageGroups.svelte';
    import ProfileImageAlbum from './ProfileImageAlbum.svelte';
    import ProfileImageViewer from './ProfileImageViewer.svelte';
    let {
        client,
        groups,
        initialAlbum,
        directView,
        onclose,
        onimagechange,
        covered = false,
    }: {
        client: LorepiaClient;
        groups: ProfileImageGroup[];
        initialAlbum?: { title: string; images: ProfileImage[] };
        directView?: { images: ProfileImage[]; assetId: string };
        onclose: () => void;
        onimagechange?: (assetId: string) => void;
        covered?: boolean;
    } = $props();
    let album = $state(untrack(() => initialAlbum));
    let viewer = $state(untrack(() => directView));
    let albumTrigger: HTMLButtonElement | undefined;
    let viewerTrigger: HTMLButtonElement | undefined;
    function restore(trigger?: HTMLButtonElement) {
        void tick().then(() =>
            requestAnimationFrame(() => {
                if (trigger?.isConnected && !trigger.closest('[inert]'))
                    trigger.focus({ preventScroll: true });
            }),
        );
    }
    function closeAlbum() {
        album = undefined;
        restore(albumTrigger);
    }
    function closeViewer() {
        if (directView) onclose();
        else {
            viewer = undefined;
            restore(viewerTrigger);
        }
    }
</script>

{#if !directView}
    <SettingsPanel
        title={$tr('navigation.profileImages')}
        kind="profile-resource"
        {onclose}
        covered={covered || !!album || !!viewer}
    >
        <ProfileImageGroups
            {groups}
            {client}
            showTitle={false}
            onopen={(detail: ProfileResourceDetail, trigger: HTMLButtonElement) => {
                albumTrigger = trigger;
                album = { title: detail.title, images: detail.images ?? [] };
            }}
        />
    </SettingsPanel>
    {#if album}
        <SettingsPanel
            title={album.title}
            kind="profile-resource"
            onclose={closeAlbum}
            covered={covered || !!viewer}
        >
            <ProfileImageAlbum
                {client}
                images={album.images}
                onview={(image: ProfileImage, trigger: HTMLButtonElement) => {
                    viewerTrigger = trigger;
                    viewer = { images: album?.images ?? [], assetId: image.assetId };
                }}
            />
        </SettingsPanel>
    {/if}
{/if}
{#if viewer && !covered}
    <ProfileImageViewer
        {client}
        images={viewer.images}
        initialAssetId={viewer.assetId}
        onclose={closeViewer}
        onimagechange={directView ? onimagechange : undefined}
    />
{/if}
