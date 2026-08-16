<script lang="ts">
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
        <div>
            <p class="eyebrow">Conversations</p>
            <h2 id="conversation-title">대화</h2>
        </div>
        <button
            class="compact"
            type="button"
            disabled={state.selected_character === null || state.greeting_catalog.phase !== 'ready'}
            onclick={() => void openNewConversation()}
        >
            새 대화
        </button>
    </header>

    {#if state.selected_character !== null}
        {#if state.greeting_catalog.phase === 'loading'}
            <p class="greeting-status" role="status">시작 인사 ID를 불러오는 중입니다.</p>
        {:else if state.greeting_catalog.phase === 'error'}
            <p class="greeting-status error" role="alert">
                {state.greeting_catalog.error}
            </p>
        {:else if state.greeting_catalog.value !== null}
            {@const enabledGreetings = state.greeting_catalog.value.greetings.filter(
                (greeting) => greeting.enabled,
            )}
            <div class="greeting-picker">
                <label for="conversation-greeting-selector">시작 인사</label>
                <select
                    id="conversation-greeting-selector"
                    value={state.greeting_catalog.selected_greeting_id ?? ''}
                    disabled={enabledGreetings.length === 0}
                    onchange={(event) => controller.selectGreeting(event.currentTarget.value)}
                >
                    {#if enabledGreetings.length === 0}
                        <option value="">사용 가능한 시작 인사 없음</option>
                    {/if}
                    {#each state.greeting_catalog.value.greetings as greeting (greeting.id)}
                        <option value={greeting.id} disabled={!greeting.enabled}>
                            {greeting.id} · {greeting.kind === 'default'
                                ? '기본'
                                : '대체'}{greeting.enabled ? '' : ' · 비활성'}
                        </option>
                    {/each}
                </select>
                <small> 인사 본문은 UI로 전달하지 않으며 ID와 종류만 선택합니다. </small>
            </div>
        {/if}
    {/if}

    {#if state.selected_character === null}
        <div class="state-panel empty">
            <strong>캐릭터를 선택하세요.</strong>
            <p>서재에서 캐릭터를 고르면 저장된 대화를 볼 수 있습니다.</p>
        </div>
    {:else if state.conversations.phase === 'loading'}
        <div class="state-panel" role="status">대화를 불러오는 중입니다.</div>
    {:else if state.conversations.phase === 'error'}
        <div class="state-panel error" role="alert">{state.conversations.error}</div>
    {:else if state.conversations.items.length === 0}
        <div class="state-panel empty">
            <strong>저장된 대화가 없습니다.</strong>
            <button
                class="primary"
                type="button"
                disabled={state.greeting_catalog.phase !== 'ready'}
                onclick={() => void openNewConversation()}
            >
                대화 시작
            </button>
        </div>
    {:else}
        <ul class="entity-list" aria-label={`${state.selected_character.name} 대화 목록`}>
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
        padding: 12px 20px;
        border-bottom: 1px solid var(--line);
    }

    .greeting-picker label {
        font-size: 0.78rem;
        font-weight: 750;
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
        color: var(--muted);
        font-size: 0.75rem;
    }

    .greeting-status {
        margin: 0;
        padding: 10px 20px;
        border-bottom: 1px solid var(--line);
    }

    .greeting-status.error {
        color: var(--danger);
    }
</style>
