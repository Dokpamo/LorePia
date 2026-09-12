<script lang="ts">
    import { Plus, Search, Users } from '@lucide/svelte';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { SampleCharacter } from '../workspace/view-types';
    import ProfileThumbnail from './ProfileThumbnail.svelte';
    import IconButton from '../workspace/IconButton.svelte';
    import LibraryScreen from './LibraryScreen.svelte';
    import LibrarySort from './LibrarySort.svelte';
    import { queryCharacterLibrary, type CharacterLibrarySort } from './character-library-query';
    import { tr } from '../../lib/i18n';
    import LoadingState from '../workspace/LoadingState.svelte';
    let {
        characters,
        client,
        ready,
        loaded = ready,
        loading = !loaded,
        onselect,
        onadd,
        ondetail,
    }: {
        characters: SampleCharacter[];
        client: LorepiaClient;
        ready: boolean;
        loaded?: boolean;
        loading?: boolean;
        onselect: (id: string) => void;
        onadd: () => void;
        ondetail: (active: boolean) => void;
    } = $props();
    let query = $state('');
    let sort = $state<CharacterLibrarySort>('newest');
    const library = $derived(queryCharacterLibrary(characters, '', sort));
    const filtered = $derived(query ? queryCharacterLibrary(characters, query, sort) : library);
</script>

{#snippet cards(items: SampleCharacter[])}
    <div class="seed-character-grid">
        {#each items as item (item.id)}
            <button
                class="seed-character-card ui-pressable"
                data-press-feedback="scale"
                aria-label={$tr('uiPreview.cardSelect', { name: item.name })}
                onclick={() => onselect(item.id)}
            >
                <span class="ui-press-visual">
                    <span class="seed-character-thumbnail">
                        {#if item.avatarAssetId}<ProfileThumbnail
                                {client}
                                name={item.name}
                                assetId={item.avatarAssetId}
                            />{:else}<span>{item.thumbnail}</span>{/if}
                    </span>
                    <strong>{item.name}</strong>
                </span>
            </button>
        {/each}
    </div>
{/snippet}

<LibraryScreen
    title={$tr('navigation.home')}
    searchLabel={$tr('navigation.searchCharacters')}
    bind:query
    {ondetail}
>
    {#snippet actions()}
        <IconButton label={$tr('navigation.addCharacter')} onclick={onadd} disabled={!ready}
            ><Plus /></IconButton
        >
        <LibrarySort bind:value={sort} label={$tr('navigation.sortCharacters')} />
    {/snippet}
    {#snippet results()}
        {#if loading && !characters.length}<LoadingState
                label={$tr('ux.loading.characters')}
                cards
            />
        {:else if filtered.length > 0}{@render cards(filtered)}{:else}
            <div class="seed-empty">
                <Search aria-hidden="true" />
                <h3>{$tr('navigation.noResults')}</h3>
                <p>{$tr('navigation.noResultsHint')}</p>
            </div>
        {/if}
    {/snippet}
    <div class="seed-content seed-library">
        {#if characters.length > 0}
            {@render cards(library)}
        {:else if loaded}
            <div class="seed-empty">
                <Users aria-hidden="true" />
                <h3>{$tr('workspace.emptyLibrary')}</h3>
            </div>
            <button class="seed-primary ui-pressable" disabled={!ready} onclick={onadd}
                ><span class="ui-press-visual"
                    ><Plus aria-hidden="true" />{$tr('workspace.importCard')}</span
                ></button
            >
        {:else if loading}
            <LoadingState label={$tr('ux.loading.characters')} cards />
        {/if}
    </div>
</LibraryScreen>
