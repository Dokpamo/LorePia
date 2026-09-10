<script lang="ts">
    import { Plus, Search, Users } from '@lucide/svelte';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { SampleCharacter } from '../workspace/view-types';
    import CharacterImage from '../workspace/CharacterImage.svelte';
    import IconButton from '../workspace/IconButton.svelte';
    import LibraryScreen from './LibraryScreen.svelte';
    import LibrarySort from './LibrarySort.svelte';
    import { queryCharacterLibrary, type CharacterLibrarySort } from './character-library-query';
    import { tr } from '../../lib/i18n';
    let {
        characters,
        client,
        ready,
        onselect,
        onadd,
        ondetail,
    }: {
        characters: SampleCharacter[];
        client: LorepiaClient;
        ready: boolean;
        onselect: (id: string) => void;
        onadd: () => void;
        ondetail: (active: boolean) => void;
    } = $props();
    let query = $state('');
    let sort = $state<CharacterLibrarySort>('newest');
    const filtered = $derived(queryCharacterLibrary(characters, query, sort));
    const library = $derived(queryCharacterLibrary(characters, '', sort));
</script>

{#snippet cards(items: SampleCharacter[])}
    <div class="seed-character-grid">
        {#each items as item (item.id)}
            <button
                class="seed-character-card ui-pressable"
                aria-label={$tr('uiPreview.cardSelect', { name: item.name })}
                onclick={() => onselect(item.id)}
            >
                <span class="ui-press-visual">
                    <span class="seed-character-thumbnail">
                        {#if item.avatarAssetId}<CharacterImage
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
        {#if filtered.length > 0}{@render cards(filtered)}{:else}
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
        {:else}
            <div class="seed-empty">
                <Users aria-hidden="true" />
                <h3>{ready ? $tr('workspace.emptyLibrary') : $tr('workspace.loading')}</h3>
            </div>
            <button class="seed-primary ui-pressable" disabled={!ready} onclick={onadd}
                ><span class="ui-press-visual"
                    ><Plus aria-hidden="true" />{$tr('workspace.importCard')}</span
                ></button
            >
        {/if}
    </div>
</LibraryScreen>
