<script lang="ts">
    import { Search, X } from '@lucide/svelte';
    import ChoicePopover from '../../components/ChoicePopover.svelte';
    import { onMount, tick } from 'svelte';
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import { tr } from '../../lib/i18n';
    import type { ConversationDto, LorepiaClient } from '../../lib/ipc/contracts';
    import MobileAvatar from '../../components/mobile/MobileAvatar.svelte';
    import { safeConversationPreview } from './conversation-preview';

    interface Props {
        state: LorepiaAppState;
        controller: LorepiaAppController;
        client: LorepiaClient;
        onOpenChat: () => void;
    }
    let { state: appState, controller, client, onOpenChat }: Props = $props();
    let indexed = $state<ConversationDto[] | null>(null);
    let query = $state('');
    let filter = $state('all');
    let searchOpen = $state(false);
    let input = $state<HTMLInputElement>();
    let searchButton = $state<HTMLButtonElement>();
    const source = $derived(
        [...(indexed ?? appState.conversations.items)].sort(
            (a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime(),
        ),
    );
    const characters = $derived(
        appState.library.characters.filter((character) =>
            source.some((conversation) => conversation.character_id === character.id),
        ),
    );
    const visible = $derived(
        source.filter((conversation) => {
            const name = characterFor(conversation)?.name ?? '';
            return (
                (filter === 'all' || conversation.character_id === filter) &&
                `${conversation.title} ${name}`
                    .toLocaleLowerCase('ko-KR')
                    .includes(query.trim().toLocaleLowerCase('ko-KR'))
            );
        }),
    );

    onMount(() => {
        let active = true;
        void client
            .listConversations(null)
            .then((items) => {
                if (active) indexed = items;
            })
            .catch(() => {
                if (active) indexed = appState.conversations.items;
            });
        return () => {
            active = false;
        };
    });
    function characterFor(conversation: ConversationDto) {
        return (
            appState.library.characters.find(
                (character) => character.id === conversation.character_id,
            ) ??
            (appState.selected_character?.id === conversation.character_id
                ? appState.selected_character
                : null)
        );
    }
    async function openConversation(conversation: ConversationDto): Promise<void> {
        const character = characterFor(conversation);
        if (character !== null && appState.selected_character?.id !== character.id)
            await controller.selectCharacter(character);
        if (await controller.selectConversation(conversation)) onOpenChat();
    }
    async function openSearch(): Promise<void> {
        searchOpen = true;
        await tick();
        input?.focus();
    }
    async function closeSearch(): Promise<void> {
        searchOpen = false;
        query = '';
        await tick();
        searchButton?.focus();
    }
    function dateLabel(value: string): string {
        const date = new Date(value);
        return Number.isNaN(date.getTime())
            ? ''
            : new Intl.DateTimeFormat('ko-KR', { month: 'numeric', day: 'numeric' }).format(date);
    }
</script>

<section class="mobile-screen mobile-chat-list" aria-label={$tr('mobile.nav.chat')}>
    <header class="mobile-page-header">
        <h1>{$tr('mobile.nav.chat')}</h1>
        <button
            bind:this={searchButton}
            class="mobile-icon-button"
            type="button"
            aria-label={$tr('mobile.chat.search')}
            aria-expanded={searchOpen}
            onclick={() => void openSearch()}
        >
            <Search aria-hidden="true" />
        </button>
    </header>
    <div class="mobile-page-scroll">
        {#if searchOpen}
            <div class="mobile-search-field">
                <Search aria-hidden="true" />
                <input
                    bind:this={input}
                    bind:value={query}
                    type="search"
                    aria-label={$tr('mobile.chat.search')}
                    placeholder={$tr('mobile.chat.search_hint')}
                    onkeydown={(event) => {
                        if (event.key === 'Escape') void closeSearch();
                    }}
                />
                <button
                    class="mobile-icon-button"
                    type="button"
                    aria-label={$tr('conversation.search.close')}
                    onclick={() => void closeSearch()}><X aria-hidden="true" /></button
                >
            </div>
        {/if}
        <section class="mobile-conversations" aria-label={$tr('conversation.list.all_label')}>
            <header class="mobile-card-heading">
                <h2>{$tr('mobile.chat.recent')}</h2>
                <ChoicePopover
                    id="mobile-conversation-filter"
                    className="mobile-filter"
                    label={$tr('conversation.filter.label')}
                    value={filter}
                    showLabel={false}
                    options={[
                        { value: 'all', label: $tr('conversation.filter.all') },
                        ...characters.map((character) => ({
                            value: character.id,
                            label: character.name,
                        })),
                    ]}
                    onSelect={(value: string) => (filter = value)}
                />
            </header>
            {#if visible.length === 0}
                <div class="mobile-empty-state">
                    <p>
                        {query || filter !== 'all'
                            ? $tr('mobile.chat.no_results')
                            : $tr('mobile.chat.empty')}
                    </p>
                    {#if query || filter !== 'all'}
                        <button
                            class="mobile-button"
                            type="button"
                            onclick={() => {
                                query = '';
                                filter = 'all';
                            }}>{$tr('library.search.clear')}</button
                        >
                    {/if}
                </div>
            {:else}
                <ul class="mobile-card-list">
                    {#each visible as conversation (conversation.id)}
                        {@const character = characterFor(conversation)}
                        <li>
                            <button
                                class="mobile-conversation-row"
                                type="button"
                                onclick={() => void openConversation(conversation)}
                            >
                                <MobileAvatar {client} {character} />
                                <span class="mobile-list-copy">
                                    <span class="mobile-conversation-title">
                                        <strong
                                            >{conversation.title.trim() === ''
                                                ? (character?.name ?? $tr('app.tab.conversations'))
                                                : conversation.title}</strong
                                        >
                                        <time datetime={conversation.updated_at}
                                            >{dateLabel(conversation.updated_at)}</time
                                        >
                                    </span>
                                    <small
                                        >{safeConversationPreview(
                                            conversation,
                                            appState,
                                            $tr('conversation.list.preview', {
                                                name: character?.name ?? '',
                                            }),
                                        )}</small
                                    >
                                </span>
                            </button>
                        </li>
                    {/each}
                </ul>
            {/if}
        </section>
    </div>
</section>
