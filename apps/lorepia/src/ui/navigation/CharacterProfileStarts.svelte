<script lang="ts">
    import { ChevronRight } from '@lucide/svelte';
    import { tr, locale } from '../../lib/i18n';
    import { openingLanguage } from '../../lib/i18n/opening-language';
    import type { CharacterGreetingCatalogDto } from '../../lib/ipc/contracts';
    import type { CharacterProfilePresentation } from './character-profile-types';
    import { openingGroups, preferredOpening, type ProfileOpeningGroup } from './profile-openings';
    import ProfileMoreButton from './ProfileMoreButton.svelte';
    let {
        greetings,
        selectedId,
        presentation,
        onread,
        loading = false,
        fullPage = false,
        onshowall,
    }: {
        greetings: CharacterGreetingCatalogDto['greetings'];
        selectedId: string | null;
        presentation?: CharacterProfilePresentation;
        onread: (group: ProfileOpeningGroup, trigger: HTMLButtonElement) => void;
        loading?: boolean;
        fullPage?: boolean;
        onshowall?: (trigger: HTMLButtonElement) => void;
    } = $props();
    const groups = $derived(openingGroups(greetings, presentation));
    const visible = $derived(fullPage ? groups : groups.slice(0, 3));
</script>

<section
    class="seed-profile-starts"
    data-full-page={fullPage}
    aria-label={$tr('navigation.profileIntroduction')}
>
    {#if !fullPage}<h2>{$tr('navigation.profileIntroduction')}</h2>{/if}
    <p class="seed-profile-caption">{$tr('navigation.startReadHint')}</p>
    {#each visible as group (group.id)}
        {@const variant = preferredOpening(
            group,
            $locale,
            presentation?.recommendedLanguage,
            selectedId,
        )}
        <button
            type="button"
            class="seed-profile-start ui-pressable"
            disabled={loading}
            onclick={(event) => onread(group, event.currentTarget)}
        >
            <span class="ui-press-visual">
                <span>
                    <strong
                        >{variant.title ??
                            (group.kind === 'default'
                                ? $tr('navigation.defaultStart')
                                : $tr('navigation.numberedStart', {
                                      number: group.number,
                                  }))}</strong
                    >
                    {#if variant.body}<p>{variant.body}</p>{/if}
                    {#if group.variants.some((item) => item.language)}
                        <small
                            >{group.variants
                                .map((item) => openingLanguage(item.language))
                                .join(' · ')}</small
                        >
                    {/if}
                </span>
                <ChevronRight aria-hidden="true" />
            </span>
        </button>
    {:else}<p class="seed-profile-empty">
            {$tr(loading ? 'workspace.loading' : 'navigation.noStarts')}
        </p>{/each}
    {#if !fullPage && groups.length > 3 && onshowall}
        <ProfileMoreButton
            label={$tr('navigation.moreStarts', { number: groups.length })}
            onclick={onshowall}
        />
    {/if}
</section>
