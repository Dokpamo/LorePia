<script lang="ts">
    import { UserRound } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import CharacterImage from '../workspace/CharacterImage.svelte';
    import type { CharacterProfilePresentation } from './character-profile-types';
    import ProfileDisclosure from './ProfileDisclosure.svelte';
    import ProfileMoreButton from './ProfileMoreButton.svelte';
    let {
        client,
        description,
        presentation,
        onread,
    }: {
        client: LorepiaClient;
        description: string;
        presentation?: CharacterProfilePresentation;
        onread: (trigger: HTMLButtonElement) => void;
    } = $props();
    const note = $derived(presentation?.note?.trim() ?? '');
    const guide = $derived(presentation?.guide ?? '');
    const creatorName = $derived(presentation?.creator?.name.trim() ?? '');
    const expandable = $derived(description.length > 420);
    const tags = $derived([
        ...new Set(
            presentation?.tags?.map((tag) => tag.trim().replace(/^#+/, '')).filter(Boolean) ?? [],
        ),
    ]);
</script>

{#if tags.length}
    <ul class="seed-profile-tags" aria-label={$tr('navigation.characterTags')}>
        {#each tags as tag (tag)}<li>#{tag}</li>{/each}
    </ul>
{/if}
<div class="seed-profile-description" aria-label={$tr('navigation.workDescription')}>
    <p class:seed-profile-description-clamped={expandable}>
        {description || $tr('workspace.noDescription')}
    </p>
    {#if expandable}
        <ProfileMoreButton label={$tr('navigation.readProfile')} onclick={onread} />
    {/if}
</div>
<section class="seed-profile-author" aria-label={$tr('navigation.profileNote')}>
    <div class="seed-profile-byline" aria-label={$tr('navigation.profileCreator')}>
        <span class="seed-profile-byline-avatar">
            {#if presentation?.creator?.avatarAssetId}
                <CharacterImage
                    {client}
                    assetId={presentation.creator.avatarAssetId}
                    name={presentation.creator.name}
                />
            {:else}<UserRound aria-hidden="true" />{/if}
        </span>
        <span>{$tr('navigation.profileCreator')}</span>
        <strong>{creatorName || $tr('navigation.creatorUnavailable')}</strong>
    </div>
    <h2>{$tr('navigation.profileNote')}</h2>
    <p class:seed-profile-empty={!note}>
        {note || $tr('navigation.noteUnavailable')}
    </p>
</section>
<ProfileDisclosure title={$tr('navigation.profileGuide')} detail={$tr('navigation.guideHint')}>
    <p class:seed-profile-empty={!presentation?.guide}>
        {guide || $tr('navigation.guideUnavailable')}
    </p>
</ProfileDisclosure>
