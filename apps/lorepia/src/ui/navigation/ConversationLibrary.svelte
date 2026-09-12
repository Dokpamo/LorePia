<script lang="ts">
    import { MessageCircle } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { SampleCharacter } from '../workspace/view-types';
    import type { ConversationListItem, LibrarySortOrder } from './navigation-types';
    import { queryConversationLibrary } from './conversation-library-query';
    import LibrarySort from './LibrarySort.svelte';
    import LibraryScreen from './LibraryScreen.svelte';
    import NewConversationButton from './NewConversationButton.svelte';
    import LastChatTime from './LastChatTime.svelte';
    import LoadingState from '../workspace/LoadingState.svelte';
    let {
        conversations,
        characters,
        loading,
        error,
        onopen,
        onnew,
        onretry,
        ondetail,
    }: {
        conversations: ConversationListItem[];
        characters: SampleCharacter[];
        loading: boolean;
        error: string | null;
        onopen: (id: string) => void;
        onnew: (id: string, trigger: HTMLButtonElement) => void;
        onretry: () => void;
        ondetail: (active: boolean) => void;
    } = $props();
    let query = $state('');
    let sort = $state<LibrarySortOrder>('newest');
    const library = $derived(queryConversationLibrary(conversations, '', '', sort));
    const filtered = $derived(
        query ? queryConversationLibrary(conversations, query, '', sort) : library,
    );
</script>

{#snippet conversationRows(items: ConversationListItem[], searching: boolean)}
    {#if error}<div class="seed-inline-error" role="alert">
            <p>{error}</p>
            <button class="seed-secondary ui-pressable" onclick={onretry}
                ><span class="ui-press-visual">{$tr('workspace.retry')}</span></button
            >
        </div>{/if}
    {#if loading && !conversations.length}<LoadingState
            label={$tr('ux.loading.conversations')}
        />{/if}
    {#each items as item (item.id)}
        <div class="seed-conversation-row">
            <button
                class="seed-row ui-pressable"
                aria-label={`${item.title} · ${item.characterName}`}
                onclick={() => onopen(item.id)}
                ><span class="ui-press-visual"
                    ><span class="seed-conversation-avatar">{item.characterName.slice(0, 1)}</span
                    ><span class="seed-row-copy"
                        ><strong>{item.title}</strong><small>{item.characterName}</small></span
                    ><LastChatTime value={item.updatedAt} /></span
                ></button
            >
        </div>
    {:else}
        {#if !loading && !error}<div class="seed-empty">
                <MessageCircle aria-hidden="true" />
                <h3>{$tr(searching ? 'navigation.noResults' : 'navigation.noChats')}</h3>
                <p>
                    {$tr(searching ? 'navigation.noResultsHint' : 'navigation.noChatsHint')}
                </p>
            </div>{/if}
    {/each}
{/snippet}

<LibraryScreen
    title={$tr('navigation.chats')}
    searchLabel={$tr('navigation.searchChats')}
    regionLabel={$tr('uiPreview.history')}
    bind:query
    {ondetail}
>
    {#snippet actions()}
        <NewConversationButton {characters} onselect={onnew} />
        <LibrarySort
            bind:value={sort}
            label={$tr('navigation.sortChats')}
            nameLabel={$tr('navigation.sortTitle')}
        />
    {/snippet}
    {#snippet results()}{@render conversationRows(filtered, true)}{/snippet}
    <div class="seed-content seed-library seed-conversations">
        {@render conversationRows(library, false)}
    </div>
</LibraryScreen>
