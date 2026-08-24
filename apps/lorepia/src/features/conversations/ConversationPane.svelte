<script lang="ts">
    import { MessageSquarePlus, Search } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaAppState, LorepiaAppController } from '../../app/app-controller';
    import type { ConversationDto } from '../../lib/ipc/contracts';

    interface Props {
        state: LorepiaAppState;
        controller: LorepiaAppController;
        onOpenChat: () => void;
        rootView?: boolean;
    }

    let { state: appState, controller, onOpenChat, rootView = false }: Props = $props();
    let searchQuery = $state('');
    const visibleConversations = $derived(
        appState.conversations.items.filter((conversation) => {
            const query = searchQuery.trim().toLocaleLowerCase('ko-KR');
            if (query === '') return true;
            return `${conversation.title} ${appState.selected_character?.name ?? ''}`
                .toLocaleLowerCase('ko-KR')
                .includes(query);
        }),
    );

    async function selectConversation(conversation: ConversationDto): Promise<void> {
        if (await controller.selectConversation(conversation)) onOpenChat();
    }

    async function openNewConversation(): Promise<void> {
        if (await controller.openNewConversation()) onOpenChat();
    }

    function relativeDate(value: string): string {
        const parsed = new Date(value);
        return Number.isNaN(parsed.getTime())
            ? ''
            : new Intl.DateTimeFormat('ko-KR', { month: 'short', day: 'numeric' }).format(parsed);
    }
</script>

<section
    class="pane conversation-pane"
    class:root-view={rootView}
    aria-labelledby={rootView ? 'conversation-root-title' : 'conversation-title'}
>
    {#if rootView}
        <header class="mobile-top-bar mobile-root-header conversation-root-header">
            <h1 id="conversation-root-title">채팅</h1>
        </header>
        <label class="conversation-search mobile-root-search">
            <Search class="conversation-search-icon" aria-hidden="true" />
            <span class="sr-only">대화 검색</span>
            <input
                type="search"
                aria-label="대화 검색"
                placeholder="대화 검색"
                bind:value={searchQuery}
            />
        </label>
    {/if}
    <header class="pane-header">
        <h2 id="conversation-title" class="sr-only">{$tr('conversation.title')}</h2>
        {#if !rootView || appState.selected_character !== null}
            <button
                class="compact new-conversation-button"
                class:mobile-root-fab={rootView}
                type="button"
                disabled={appState.selected_character === null ||
                    appState.greeting_catalog.phase !== 'ready'}
                onclick={() => void openNewConversation()}
            >
                <span
                    class="new-conversation-mark"
                    class:mobile-root-fab-mark={rootView}
                    aria-hidden="true"
                >
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

    {#if appState.selected_character === null}
        {#if !rootView}
            <div class="state-panel empty conversation-empty">
                <strong>{$tr('conversation.empty.title')}</strong>
            </div>
        {/if}
    {:else if appState.conversations.phase === 'loading'}
        <div class="state-panel" role="status">{$tr('conversation.loading')}</div>
    {:else if appState.conversations.phase === 'error'}
        <div class="state-panel error" role="alert">{appState.conversations.error}</div>
    {:else if appState.conversations.items.length === 0}
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
            class="entity-list"
            aria-label={$tr('conversation.list.label', { name: appState.selected_character.name })}
        >
            {#each visibleConversations as conversation (conversation.id)}
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
                                >{appState.selected_character.name.slice(0, 1)}</span
                            >
                            <span class="entity-copy conversation-copy">
                                <span class="conversation-line">
                                    <strong
                                        >{conversation.title ||
                                            appState.selected_character.name}</strong
                                    >
                                    <time datetime={conversation.updated_at}
                                        >{relativeDate(conversation.updated_at)}</time
                                    >
                                </span>
                                <span>{appState.selected_character.name}과의 대화</span>
                            </span>
                        {:else}
                            <span class="entity-copy">
                                <strong
                                    >{conversation.title ||
                                        appState.selected_character.name}</strong
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

    .conversation-pane.root-view .entity-list {
        padding: 0 8px calc(var(--mobile-nav) + 92px + env(safe-area-inset-bottom));
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
</style>
