<script lang="ts">
    import { MessageSquarePlus, Search, X } from '@lucide/svelte';
    import { onMount, tick } from 'svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaAppState, LorepiaAppController } from '../../app/app-controller';
    import type { CharacterDto, ConversationDto, LorepiaClient } from '../../lib/ipc/contracts';

    interface Props {
        state: LorepiaAppState;
        controller: LorepiaAppController;
        client?: Pick<LorepiaClient, 'listConversations'>;
        onOpenChat: () => void;
        rootView?: boolean;
    }

    let { state: appState, controller, client, onOpenChat, rootView = false }: Props = $props();
    let searchQuery = $state('');
    let searchOpen = $state(false);
    let searchInput = $state<HTMLInputElement | null>(null);
    let conversationFilter = $state('all');
    let indexedConversations = $state<ConversationDto[] | null>(null);

    const sourceConversations = $derived.by(() => {
        const source =
            rootView && indexedConversations !== null
                ? indexedConversations
                : appState.conversations.items;
        return [...source].sort(
            (left, right) =>
                new Date(right.updated_at).getTime() - new Date(left.updated_at).getTime(),
        );
    });
    const conversationFilters = $derived.by(() => {
        const characterIds = new Set(
            sourceConversations.map((conversation) => conversation.character_id),
        );
        return appState.library.characters.filter((character) => characterIds.has(character.id));
    });
    const visibleConversations = $derived(
        sourceConversations.filter((conversation) => {
            if (conversationFilter !== 'all' && conversation.character_id !== conversationFilter) {
                return false;
            }
            const query = searchQuery.trim().toLocaleLowerCase('ko-KR');
            if (query === '') return true;
            return `${conversation.title} ${characterFor(conversation)?.name ?? ''}`
                .toLocaleLowerCase('ko-KR')
                .includes(query);
        }),
    );

    onMount(() => {
        if (!rootView || typeof client?.listConversations !== 'function') return;
        let active = true;
        void client
            .listConversations(null)
            .then((conversations) => {
                if (active) indexedConversations = conversations;
            })
            .catch(() => {
                if (active) indexedConversations = appState.conversations.items;
            });
        return () => {
            active = false;
        };
    });

    function characterFor(conversation: ConversationDto): CharacterDto | null {
        return (
            appState.library.characters.find(
                (character) => character.id === conversation.character_id,
            ) ??
            (appState.selected_character?.id === conversation.character_id
                ? appState.selected_character
                : null)
        );
    }

    function conversationTitle(
        conversation: ConversationDto,
        character: CharacterDto | null,
    ): string {
        const title = conversation.title.trim();
        if (title !== '') return title;
        return character?.name ?? $tr('conversation.title');
    }

    async function toggleSearch(): Promise<void> {
        searchOpen = !searchOpen;
        if (!searchOpen) {
            searchQuery = '';
            return;
        }
        await tick();
        searchInput?.focus();
    }

    function closeSearch(): void {
        searchOpen = false;
        searchQuery = '';
    }

    function handleSearchKeydown(event: KeyboardEvent): void {
        if (event.key !== 'Escape') return;
        event.preventDefault();
        closeSearch();
    }

    async function selectConversation(conversation: ConversationDto): Promise<void> {
        const character = characterFor(conversation);
        if (character !== null && appState.selected_character?.id !== character.id) {
            await controller.selectCharacter(character);
        }
        if (await controller.selectConversation(conversation)) onOpenChat();
    }

    async function openNewConversation(): Promise<void> {
        if (await controller.openNewConversation()) onOpenChat();
    }

    function relativeDate(value: string): string {
        const parsed = new Date(value);
        if (Number.isNaN(parsed.getTime())) return '';
        const now = new Date();
        const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate());
        const startOfValue = new Date(parsed.getFullYear(), parsed.getMonth(), parsed.getDate());
        const dayDelta = Math.round(
            (startOfToday.getTime() - startOfValue.getTime()) / (24 * 60 * 60 * 1000),
        );
        if (dayDelta === 0) {
            return new Intl.DateTimeFormat('ko-KR', {
                hour: 'numeric',
                minute: '2-digit',
            }).format(parsed);
        }
        if (dayDelta === 1) return $tr('conversation.list.yesterday');
        return new Intl.DateTimeFormat('ko-KR', {
            ...(parsed.getFullYear() === now.getFullYear() ? {} : { year: 'numeric' as const }),
            month: 'short',
            day: 'numeric',
        }).format(parsed);
    }

    function conversationPreview(conversation: ConversationDto): string {
        if (
            appState.selected_conversation?.id === conversation.id &&
            appState.messages.phase === 'ready'
        ) {
            const lastMessage = appState.messages.items.at(-1)?.content.trim();
            if (lastMessage) return lastMessage.replace(/\s+/g, ' ');
        }
        return $tr('conversation.list.preview', {
            name: characterFor(conversation)?.name ?? '',
        });
    }
</script>

<section
    class="pane conversation-pane"
    class:root-view={rootView}
    aria-labelledby={rootView ? 'conversation-root-title' : 'conversation-title'}
>
    {#if rootView}
        <header class="mobile-top-frame mobile-root-header conversation-root-header">
            <h1 id="conversation-root-title">채팅</h1>
            <div class="mobile-root-actions" aria-label="채팅 작업">
                <button
                    class="mobile-top-action conversation-search-shortcut"
                    type="button"
                    aria-label={searchOpen
                        ? $tr('conversation.search.close')
                        : $tr('conversation.search.open')}
                    aria-pressed={searchOpen}
                    onclick={() => void toggleSearch()}
                >
                    {#if searchOpen}
                        <X aria-hidden="true" />
                    {:else}
                        <Search aria-hidden="true" />
                    {/if}
                </button>
                {#if appState.selected_character !== null}
                    <button
                        class="mobile-top-action mobile-top-add-action new-conversation-top-action"
                        type="button"
                        aria-label={$tr('conversation.new')}
                        disabled={appState.greeting_catalog.phase !== 'ready'}
                        onclick={() => void openNewConversation()}
                    >
                        <MessageSquarePlus aria-hidden="true" />
                    </button>
                {/if}
            </div>
        </header>
        <div
            class="conversation-filter-strip"
            role="tablist"
            aria-label={$tr('conversation.filter.label')}
        >
            <button
                class="conversation-filter-pill"
                class:active={conversationFilter === 'all'}
                type="button"
                role="tab"
                aria-selected={conversationFilter === 'all'}
                aria-controls="conversation-filtered-list"
                onclick={() => (conversationFilter = 'all')}
            >
                {$tr('conversation.filter.all')}
            </button>
            {#each conversationFilters as character (character.id)}
                <button
                    class="conversation-filter-pill"
                    class:active={conversationFilter === character.id}
                    type="button"
                    role="tab"
                    aria-selected={conversationFilter === character.id}
                    aria-controls="conversation-filtered-list"
                    onclick={() => (conversationFilter = character.id)}
                >
                    {character.name}
                </button>
            {/each}
        </div>
        {#if searchOpen}
            <label class="conversation-search mobile-root-search">
                <Search class="conversation-search-icon" aria-hidden="true" />
                <span class="sr-only">{$tr('conversation.search.open')}</span>
                <input
                    type="search"
                    aria-label={$tr('conversation.search.open')}
                    placeholder={$tr('conversation.search.open')}
                    bind:this={searchInput}
                    bind:value={searchQuery}
                    onkeydown={handleSearchKeydown}
                />
            </label>
        {/if}
    {/if}
    <header class="pane-header">
        <h2 id="conversation-title" class="sr-only">{$tr('conversation.title')}</h2>
        {#if !rootView && appState.selected_character !== null}
            <button
                class="compact new-conversation-button"
                type="button"
                disabled={appState.greeting_catalog.phase !== 'ready'}
                onclick={() => void openNewConversation()}
            >
                <span class="new-conversation-mark" aria-hidden="true">
                    <MessageSquarePlus class="new-conversation-icon" />
                </span>
                <span class="new-conversation-label">{$tr('conversation.new')}</span>
            </button>
        {/if}
    </header>

    {#if !rootView && appState.selected_character !== null}
        {#if appState.greeting_catalog.phase === 'loading'}
            <p class="greeting-status" role="status">{$tr('conversation.greeting.loading')}</p>
        {:else if appState.greeting_catalog.phase === 'error'}
            <p class="greeting-status error" role="alert">
                {appState.greeting_catalog.error}
            </p>
        {:else if appState.greeting_catalog.value !== null}
            {@const enabledGreetings = appState.greeting_catalog.value.greetings.filter(
                (greeting) => greeting.enabled,
            )}
            <div class="greeting-picker">
                <label for="conversation-greeting-selector"
                    >{$tr('conversation.greeting.label')}</label
                >
                <select
                    id="conversation-greeting-selector"
                    value={appState.greeting_catalog.selected_greeting_id ?? ''}
                    disabled={enabledGreetings.length === 0}
                    onchange={(event) => controller.selectGreeting(event.currentTarget.value)}
                >
                    {#if enabledGreetings.length === 0}
                        <option value="">{$tr('conversation.greeting.none')}</option>
                    {/if}
                    {#each appState.greeting_catalog.value.greetings as greeting (greeting.id)}
                        <option value={greeting.id} disabled={!greeting.enabled}>
                            {greeting.id} · {greeting.kind === 'default'
                                ? $tr('conversation.greeting.default')
                                : $tr('conversation.greeting.alternate')}{greeting.enabled
                                ? ''
                                : $tr('conversation.greeting.disabled')}
                        </option>
                    {/each}
                </select>
                <small>{$tr('conversation.greeting.note')}</small>
            </div>
        {/if}
    {/if}

    {#if !rootView && appState.selected_character === null}
        {#if !rootView}
            <div class="state-panel empty conversation-empty">
                <strong>{$tr('conversation.empty.title')}</strong>
            </div>
        {/if}
    {:else if !rootView && appState.conversations.phase === 'loading'}
        <div class="state-panel" role="status">{$tr('conversation.loading')}</div>
    {:else if !rootView && appState.conversations.phase === 'error'}
        <div class="state-panel error" role="alert">{appState.conversations.error}</div>
    {:else if sourceConversations.length === 0}
        {#if !rootView}
            <div class="state-panel empty conversation-empty">
                <strong>{$tr('conversation.none.title')}</strong>
                <button
                    class="primary"
                    type="button"
                    disabled={appState.greeting_catalog.phase !== 'ready'}
                    onclick={() => void openNewConversation()}
                >
                    {$tr('conversation.none.start')}
                </button>
            </div>
        {/if}
    {:else if visibleConversations.length === 0}
        <div class="state-panel empty conversation-search-empty">
            <strong>일치하는 대화가 없습니다.</strong>
            <button type="button" onclick={() => (searchQuery = '')}>검색 지우기</button>
        </div>
    {:else}
        <ul
            id={rootView ? 'conversation-filtered-list' : undefined}
            class="entity-list"
            role={rootView ? 'tabpanel' : undefined}
            aria-label={rootView
                ? $tr('conversation.list.all_label')
                : $tr('conversation.list.label', { name: appState.selected_character?.name ?? '' })}
        >
            {#each visibleConversations as conversation (conversation.id)}
                {@const conversationCharacter = characterFor(conversation)}
                <li>
                    <button
                        type="button"
                        class="entity-row conversation-row"
                        class:mobile-root-row={rootView}
                        class:active={appState.selected_conversation?.id === conversation.id}
                        aria-pressed={appState.selected_conversation?.id === conversation.id}
                        onclick={() => void selectConversation(conversation)}
                    >
                        {#if rootView}
                            <span class="avatar" aria-hidden="true"
                                >{conversationCharacter?.name.slice(0, 1) ?? '?'}</span
                            >
                            <span class="entity-copy conversation-copy">
                                <span class="conversation-line">
                                    <strong
                                        >{conversationTitle(
                                            conversation,
                                            conversationCharacter,
                                        )}</strong
                                    >
                                    <time datetime={conversation.updated_at}
                                        >{relativeDate(conversation.updated_at)}</time
                                    >
                                </span>
                                <span>{conversationPreview(conversation)}</span>
                            </span>
                        {:else}
                            <span class="entity-copy">
                                <strong
                                    >{conversationTitle(
                                        conversation,
                                        appState.selected_character,
                                    )}</strong
                                >
                                <span>{relativeDate(conversation.updated_at)}</span>
                            </span>
                        {/if}
                    </button>
                </li>
            {/each}
        </ul>
    {/if}
</section>

<style>
    .new-conversation-mark {
        display: none;
    }

    .new-conversation-mark :global(.new-conversation-icon) {
        width: 24px;
        height: 24px;
    }

    .conversation-pane.root-view .pane-header {
        min-height: 0;
        padding: 0;
    }

    .conversation-filter-strip {
        display: flex;
        width: min(100%, var(--reading));
        min-height: clamp(39px, 15.561vw, 68px);
        flex: none;
        align-items: center;
        padding: clamp(4px, 1.831vw, 8px) max(var(--mobile-top-inset), env(safe-area-inset-right))
            clamp(6px, 2.288vw, 10px) max(var(--mobile-top-inset), env(safe-area-inset-left));
        gap: clamp(6px, 2.288vw, 10px);
        margin-inline: auto;
        overflow-x: auto;
        overscroll-behavior-x: contain;
        scrollbar-width: none;
        scroll-snap-type: x proximity;
    }

    .conversation-filter-strip::-webkit-scrollbar {
        display: none;
    }

    .conversation-filter-pill {
        display: inline-flex;
        height: var(--mobile-pill-control);
        min-height: var(--mobile-pill-control);
        min-width: max-content;
        flex: none;
        align-items: center;
        justify-content: center;
        padding: 0 clamp(11px, 4.577vw, 20px);
        border: 1px solid var(--line);
        border-radius: var(--radius-pill);
        background: var(--surface-raised);
        box-shadow: none;
        color: var(--ink);
        font-size: clamp(10px, 4.119vw, 18px);
        font-weight: 700;
        letter-spacing: -0.015em;
        scroll-snap-align: start;
        transition:
            background-color 140ms ease,
            border-color 140ms ease,
            color 140ms ease,
            transform 140ms ease;
    }

    :global(.app-shell[data-layout='mobile']) .conversation-filter-strip {
        min-height: clamp(37px, 15.561vw, 68px);
        padding: clamp(4px, 1.831vw, 8px) max(var(--mobile-top-inset), env(safe-area-inset-right))
            clamp(5px, 2.288vw, 10px) max(var(--mobile-top-inset), env(safe-area-inset-left));
        gap: clamp(5px, 2.288vw, 10px);
    }

    :global(.app-shell[data-layout='mobile']) .conversation-filter-pill {
        padding-inline: clamp(11px, 4.577vw, 20px);
        font-size: clamp(10px, 4.119vw, 18px);
    }

    .conversation-filter-pill.active {
        border-color: var(--ink);
        background: var(--ink);
        color: var(--bg);
    }

    .conversation-filter-pill:active {
        transform: scale(0.97);
    }

    .conversation-filter-pill:focus-visible {
        outline: 2px solid var(--accent);
        outline-offset: 0;
    }

    .conversation-search-shortcut[aria-pressed='true'] {
        background: var(--surface-active);
    }

    .conversation-search {
        animation: conversation-search-in 160ms ease-out both;
    }

    .conversation-pane.root-view .entity-list {
        padding: 8px 8px calc(var(--mobile-nav) + 20px + env(safe-area-inset-bottom));
        gap: 0;
    }

    .conversation-copy {
        grid-auto-flow: row;
        align-items: stretch;
        justify-content: initial;
        gap: 3px;
        grid-template-columns: minmax(0, 1fr);
    }

    .conversation-line {
        display: flex;
        min-width: 0;
        align-items: baseline;
        justify-content: space-between;
        color: var(--ink);
        gap: 10px;
    }

    .conversation-line strong {
        overflow: hidden;
        font-size: 1.0625rem;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .conversation-line time {
        flex: none;
        color: var(--ink-muted);
        font-size: 0.75rem;
    }

    .conversation-search-empty {
        display: grid;
        justify-items: center;
        gap: 10px;
    }

    .greeting-picker {
        display: grid;
        gap: 6px;
        padding: 8px 12px 10px;
        border-bottom: 1px solid var(--line);
    }

    .greeting-picker label {
        font-size: 0.6875rem;
        font-weight: 600;
    }

    .greeting-picker select {
        width: 100%;
        min-width: 0;
        padding-inline: 10px;
        border: 1px solid var(--line);
        border-radius: var(--radius-sm);
        color: var(--ink);
        background: var(--surface-raised);
    }

    .greeting-picker small,
    .greeting-status {
        color: var(--ink-muted);
        font-size: 0.6875rem;
    }

    .greeting-status {
        margin: 0;
        padding: 8px 12px;
        border-bottom: 1px solid var(--line);
    }

    .greeting-status.error {
        color: var(--danger);
    }

    :global(.app-shell[data-layout='mobile']) .conversation-pane.root-view .entity-list {
        padding-inline: 0;
        padding-top: clamp(3px, 1.831vw, 8px);
        padding-bottom: calc(
            var(--mobile-nav) + clamp(8px, 4.577vw, 20px) + env(safe-area-inset-bottom)
        );
    }

    :global(.app-shell[data-layout='mobile']) .conversation-pane.root-view .mobile-root-row {
        min-height: clamp(46px, 19.222vw, 84px);
        padding: clamp(4px, 1.831vw, 8px) clamp(10px, 4.119vw, 18px);
        border-radius: 0;
        gap: clamp(8px, 3.204vw, 14px);
    }

    :global(.app-shell[data-layout='mobile']) .conversation-pane.root-view .mobile-root-row.active {
        background: transparent;
        color: var(--ink);
        font-weight: 400;
    }

    :global(.app-shell[data-layout='mobile'])
        .conversation-pane.root-view
        .mobile-root-row
        .avatar {
        width: clamp(35px, 14.645vw, 64px);
        height: clamp(35px, 14.645vw, 64px);
        background: var(--surface-active);
        color: var(--ink);
        font-size: clamp(11px, 4.577vw, 20px);
        font-weight: 700;
    }

    :global(.app-shell[data-layout='mobile'])
        .conversation-pane.root-view
        .conversation-line
        strong {
        font-size: clamp(11px, 4.577vw, 20px);
        font-weight: 700;
    }

    :global(.app-shell[data-layout='mobile'])
        .conversation-pane.root-view
        .conversation-copy
        > span:last-child {
        display: -webkit-box;
        font-size: clamp(9px, 3.661vw, 16px);
        line-height: 1.35;
        white-space: normal;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 2;
        line-clamp: 2;
    }

    :global(.app-shell[data-layout='mobile']) .conversation-pane.root-view .conversation-line time {
        font-size: clamp(7px, 2.975vw, 13px);
        font-variant-numeric: tabular-nums;
    }

    @media (max-width: 719px) {
        :global(.navigator) .pane-header .new-conversation-button {
            position: absolute;
            z-index: 21;
            right: 22px;
            bottom: calc(var(--mobile-nav) + 26px + env(safe-area-inset-bottom));
            display: grid;
            width: 54px;
            height: 54px;
            min-height: 54px;
            padding: 0;
            border: 0;
            border-radius: 50%;
            background: var(--primary-bg);
            box-shadow: var(--shadow-2);
            color: var(--primary-ink);
            place-items: center;
        }

        @media (hover: hover) and (pointer: fine) {
            :global(.navigator) .pane-header .new-conversation-button:hover:not(:disabled) {
                background: var(--primary-bg-hover);
            }
        }

        :global(.navigator) .pane-header .new-conversation-button:disabled {
            opacity: 0.45;
        }

        .new-conversation-mark {
            display: grid;
            place-items: center;
        }

        .new-conversation-label {
            position: absolute;
            overflow: hidden;
            width: 1px;
            height: 1px;
            padding: 0;
            border: 0;
            margin: -1px;
            clip: rect(0, 0, 0, 0);
            white-space: nowrap;
        }
    }

    @keyframes conversation-search-in {
        from {
            opacity: 0;
            transform: translateY(-4px);
        }

        to {
            opacity: 1;
            transform: translateY(0);
        }
    }

    @media (prefers-reduced-motion: reduce) {
        .conversation-filter-pill,
        .conversation-search {
            animation: none;
            transition: none;
        }
    }
</style>
