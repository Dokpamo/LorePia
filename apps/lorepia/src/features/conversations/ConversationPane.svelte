<script lang="ts">
    import { tr } from '../../lib/i18n';
    import type { LorepiaAppState, LorepiaAppController } from '../../app/app-controller';
    import type { ConversationDto } from '../../lib/ipc/contracts';

    interface Props {
        state: LorepiaAppState;
        controller: LorepiaAppController;
        onOpenChat: () => void;
    }

    let { state, controller, onOpenChat }: Props = $props();

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

<section class="pane conversation-pane" aria-labelledby="conversation-title">
    <header class="pane-header">
        <h2 id="conversation-title" class="sr-only">{$tr('conversation.title')}</h2>
        <button
            class="compact"
            type="button"
            disabled={state.selected_character === null || state.greeting_catalog.phase !== 'ready'}
            onclick={() => void openNewConversation()}
        >
            {$tr('conversation.new')}
        </button>
    </header>

    {#if state.selected_character !== null}
        {#if state.greeting_catalog.phase === 'loading'}
            <p class="greeting-status" role="status">{$tr('conversation.greeting.loading')}</p>
        {:else if state.greeting_catalog.phase === 'error'}
            <p class="greeting-status error" role="alert">
                {state.greeting_catalog.error}
            </p>
        {:else if state.greeting_catalog.value !== null}
            {@const enabledGreetings = state.greeting_catalog.value.greetings.filter(
                (greeting) => greeting.enabled,
            )}
            <div class="greeting-picker">
                <label for="conversation-greeting-selector"
                    >{$tr('conversation.greeting.label')}</label
                >
                <select
                    id="conversation-greeting-selector"
                    value={state.greeting_catalog.selected_greeting_id ?? ''}
                    disabled={enabledGreetings.length === 0}
                    onchange={(event) => controller.selectGreeting(event.currentTarget.value)}
                >
                    {#if enabledGreetings.length === 0}
                        <option value="">{$tr('conversation.greeting.none')}</option>
                    {/if}
                    {#each state.greeting_catalog.value.greetings as greeting (greeting.id)}
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

    {#if state.selected_character === null}
        <div class="state-panel empty">
            <strong>{$tr('conversation.empty.title')}</strong>
            <p>{$tr('conversation.empty.hint')}</p>
        </div>
    {:else if state.conversations.phase === 'loading'}
        <div class="state-panel" role="status">{$tr('conversation.loading')}</div>
    {:else if state.conversations.phase === 'error'}
        <div class="state-panel error" role="alert">{state.conversations.error}</div>
    {:else if state.conversations.items.length === 0}
        <div class="state-panel empty">
            <strong>{$tr('conversation.none.title')}</strong>
            <button
                class="primary"
                type="button"
                disabled={state.greeting_catalog.phase !== 'ready'}
                onclick={() => void openNewConversation()}
            >
                {$tr('conversation.none.start')}
            </button>
        </div>
    {:else}
        <ul
            class="entity-list"
            aria-label={$tr('conversation.list.label', { name: state.selected_character.name })}
        >
            {#each state.conversations.items as conversation (conversation.id)}
                <li>
                    <button
                        type="button"
                        class="entity-row conversation-row"
                        class:active={state.selected_conversation?.id === conversation.id}
                        aria-pressed={state.selected_conversation?.id === conversation.id}
                        onclick={() => void selectConversation(conversation)}
                    >
                        <span class="entity-copy">
                            <strong>{conversation.title || state.selected_character.name}</strong>
                            <span>{relativeDate(conversation.updated_at)}</span>
                        </span>
                        <span aria-hidden="true">›</span>
                    </button>
                </li>
            {/each}
        </ul>
    {/if}
</section>

<style>
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
</style>
