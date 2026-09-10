<script lang="ts">
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { ProfileImageGroup, ProfileResourceDetail } from './character-profile-types';
    import ProfileThumbnail from './ProfileThumbnail.svelte';
    let {
        groups,
        client,
        onopen,
        showTitle = true,
        groupByKind = true,
    }: {
        groups: ProfileImageGroup[];
        showTitle?: boolean;
        groupByKind?: boolean;
        client: LorepiaClient;
        onopen: (detail: ProfileResourceDetail, trigger: HTMLButtonElement) => void;
    } = $props();
    const sections = $derived(
        groupByKind
            ? [
                  { kind: 'person', title: $tr('navigation.imagePeople') },
                  { kind: 'place', title: $tr('navigation.imagePlaces') },
                  { kind: 'item', title: $tr('navigation.imageItems') },
                  { kind: 'other', title: $tr('navigation.imageOther') },
              ]
                  .map((section) => ({
                      ...section,
                      groups: groups.filter((group) => group.kind === section.kind),
                  }))
                  .filter((section) => section.groups.length)
            : groups.length
              ? [{ kind: 'other', title: '', groups }]
              : [],
    );
</script>

<section class="seed-profile-image-groups" aria-label={$tr('navigation.profileImages')}>
    {#if showTitle}<h2>{$tr('navigation.profileImages')}</h2>{/if}
    {#each sections as section (section.kind)}
        <div class="seed-profile-image-category">
            {#if sections.length > 1 || section.kind !== 'other'}<h3>{section.title}</h3>{/if}
            <div class="seed-profile-group-grid">
                {#each section.groups as group (group.id)}
                    <button
                        class="ui-pressable"
                        aria-label={$tr('navigation.imageGroupOpen', {
                            name: group.title,
                            number: group.images.length,
                        })}
                        onclick={(event) =>
                            onopen(
                                {
                                    title: group.title,
                                    images: group.images,
                                    assetId: group.coverAssetId,
                                },
                                event.currentTarget,
                            )}
                    >
                        <span class="ui-press-visual">
                            <span class="seed-profile-group-cover"
                                ><ProfileThumbnail
                                    {client}
                                    assetId={group.coverAssetId}
                                    name={group.title}
                                /></span
                            >
                            <strong>{group.title}</strong><span class="seed-profile-caption"
                                >{$tr('navigation.imageCount', {
                                    number: group.images.length,
                                })}</span
                            >
                        </span>
                    </button>
                {/each}
            </div>
        </div>
    {:else}<p class="seed-profile-empty">{$tr('navigation.noImages')}</p>{/each}
</section>
