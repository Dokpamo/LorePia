<script lang="ts">
    import { FileCode, BookOpen } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { CharacterRenderProfileDto, LorepiaClient } from '../../lib/ipc/contracts';
    import type { SampleCharacter } from '../workspace/view-types';
    import type {
        CharacterProfilePresentation,
        ProfileResourceDetail,
    } from './character-profile-types';
    import ProfileImageGroups from './ProfileImageGroups.svelte';
    import ProfileMoreButton from './ProfileMoreButton.svelte';
    import { profileImageGroups } from './profile-image-groups';
    import NavigationRow from './NavigationRow.svelte';
    let {
        character,
        client,
        onopen,
        presentation,
        onprofile,
    }: {
        character: SampleCharacter;
        client: LorepiaClient;
        presentation?: CharacterProfilePresentation;
        onprofile?: (profile: CharacterRenderProfileDto) => void;
        onopen: (detail: ProfileResourceDetail, trigger: HTMLButtonElement) => void;
    } = $props();
    let profile = $state.raw<CharacterRenderProfileDto | null>(null);
    let phase = $state<'loading' | 'ready' | 'error' | 'unavailable'>('loading');
    let retry = $state(0);
    $effect(() => {
        void retry;
        const id = character.id;
        let current = true;
        profile = null;
        phase = 'loading';
        if (!client.getCharacterRenderProfile) phase = 'unavailable';
        else
            void client
                .getCharacterRenderProfile(id)
                .then((value) => {
                    if (!current) return;
                    if (value.character_id !== id) {
                        phase = 'error';
                        return;
                    }
                    profile = value;
                    phase = 'ready';
                    onprofile?.(value);
                })
                .catch(() => {
                    if (current) phase = 'error';
                });
        return () => {
            current = false;
        };
    });
    const images = $derived.by(() => {
        const assets = profile?.assets.length
            ? profile.assets
            : character.avatarAssetId
              ? [{ asset_id: character.avatarAssetId, aliases: [$tr('navigation.coverImage')] }]
              : [];
        return [
            ...new Map(
                assets
                    .filter((asset) => !asset.media_type || asset.media_type.startsWith('image/'))
                    .map((asset) => [asset.asset_id, asset]),
            ).values(),
        ];
    });
    const lore = $derived(profile?.runtime_knowledge.filter((item) => !item.folder) ?? []);
    const groups = $derived(profileImageGroups(images, presentation?.imageGroups));
    const transforms = $derived([
        ...(profile?.display_transforms ?? []),
        ...(profile?.output_transforms ?? []),
    ]);
</script>

<div class="seed-profile-resources">
    {#if phase === 'loading'}<p class="seed-profile-caption" role="status">
            {$tr('workspace.loading')}
        </p>
    {:else if phase === 'error'}
        <p class="seed-profile-empty" role="alert">{$tr('navigation.resourcesFailed')}</p>
        <button class="seed-secondary ui-pressable" onclick={() => (retry += 1)}
            ><span class="ui-press-visual">{$tr('workspace.retry')}</span></button
        >
    {/if}
    <ProfileImageGroups groups={groups.slice(0, 3)} groupByKind={false} {client} {onopen} />
    {#if groups.length}
        <ProfileMoreButton
            label={$tr('navigation.allImages')}
            onclick={(trigger: HTMLButtonElement) =>
                onopen({ title: $tr('navigation.profileImages'), imageGroups: groups }, trigger)}
        />
    {/if}
    <NavigationRow
        title={$tr('navigation.profileLorebook')}
        description={profile
            ? $tr('navigation.resourceEntries', { number: lore.length })
            : $tr('navigation.lorebookHint')}
        disabled={!profile}
        onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) => {
            if (profile)
                onopen(
                    {
                        title: $tr('navigation.profileLorebook'),
                        collection: { kind: 'lorebook', profile },
                    },
                    event.currentTarget,
                );
        }}
    >
        {#snippet prefix()}<BookOpen />{/snippet}
    </NavigationRow>
    <NavigationRow
        title={$tr('navigation.profileScripts')}
        description={profile
            ? $tr('navigation.scriptResourceCounts', {
                  scripts: profile.runtime_scripts.length,
                  rules: transforms.length,
              })
            : $tr('navigation.scriptsHint')}
        disabled={!profile}
        onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) => {
            if (profile)
                onopen(
                    {
                        title: $tr('navigation.profileScripts'),
                        collection: { kind: 'scripts', profile },
                    },
                    event.currentTarget,
                );
        }}
    >
        {#snippet prefix()}<FileCode />{/snippet}
    </NavigationRow>
</div>
