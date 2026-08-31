<script lang="ts">
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import type { MemoryRecordSourceNavigationDto, MessageDto } from '../../lib/ipc/contracts';
    import ChatMessageActions from './ChatMessageActions.svelte';
    import PortableMessage from './PortableMessage.svelte';
    import {
        formatMessageDay,
        formatMessageTime,
        messageDayKey,
        type ChatScrollLifecycle,
        type MessageCollectionSnapshot,
        type MessageMeasurementInput,
    } from './chat-scroll.svelte';
    import type { InteractionRoomCapableClient } from './interaction-room-controller';
    import type { MessageActionsState } from './message-actions.svelte';
    import type { PortableRuntimeLifecycle } from './portable-runtime-lifecycle.svelte';
    import { VIRTUAL_MESSAGE_BLOCK_PADDING, type VirtualWindow } from './virtual-window';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        desktop: boolean;
        client?: InteractionRoomCapableClient;
        messageFocusRequest?: (MemoryRecordSourceNavigationDto & { request_id: number }) | null;
        messageCollection: MessageCollectionSnapshot;
        virtualWindow: VirtualWindow;
        chatScroll: ChatScrollLifecycle;
        messageActions: MessageActionsState;
        runtime: PortableRuntimeLifecycle;
        onNotice: (message: string) => void;
        onRemove: (messageId: string) => Promise<void>;
    }

    let {
        appState,
        controller,
        desktop,
        client,
        messageFocusRequest = null,
        messageCollection,
        virtualWindow,
        chatScroll,
        messageActions,
        runtime,
        onNotice,
        onRemove,
    }: Props = $props();
    let copyNotice = '';

    const hasLiveResponse = $derived(
        appState.chat.live_assistant_message_id !== null ||
            appState.chat.streaming_text !== '' ||
            appState.chat.reasoning_text !== '',
    );
    const visibleMessages = $derived(
        messageCollection.items.slice(virtualWindow.start, virtualWindow.end),
    );

    function startsMessageDay(message: MessageDto, globalIndex: number): boolean {
        if (globalIndex === 0) return true;
        const previous = messageCollection.items[globalIndex - 1];
        return (
            previous === undefined ||
            messageDayKey(previous.created_at) !== messageDayKey(message.created_at)
        );
    }

    function measureMessage(node: HTMLElement, input: MessageMeasurementInput) {
        return chatScroll.measureMessage(node, input);
    }

    async function commitEdit(messageId: string): Promise<void> {
        const accepted = await controller.editUserMessage(messageId, messageActions.editDraft);
        if (accepted) messageActions.finishEdit();
    }

    async function copyMessage(message: MessageDto): Promise<void> {
        try {
            await navigator.clipboard.writeText(runtime.effectiveText(message));
            copyNotice = '메시지를 복사했습니다.';
        } catch {
            copyNotice = '메시지를 복사하지 못했습니다.';
        }
        onNotice(copyNotice);
    }
</script>

<ol
    class="message-list virtualized"
    aria-label="대화 메시지"
    style:padding-top={String(VIRTUAL_MESSAGE_BLOCK_PADDING + virtualWindow.topSpacer) + 'px'}
    style:padding-bottom={String(
        VIRTUAL_MESSAGE_BLOCK_PADDING + (hasLiveResponse ? 0 : virtualWindow.bottomSpacer),
    ) + 'px'}
>
    {#each visibleMessages as message, localIndex (message.id)}
        {@const globalIndex = virtualWindow.start + localIndex}
        {@const messageEditorTextareaControlId = `edit-${message.id}`}
        {@const showsDay = startsMessageDay(message, globalIndex)}
        {@const dayKey = messageDayKey(message.created_at)}
        {@const dayLabel = formatMessageDay(message.created_at)}
        {@const turnLabel =
            message.role === 'user'
                ? '내 메시지'
                : message.role === 'assistant'
                  ? '캐릭터 메시지'
                  : '시스템 메시지'}
        {#if showsDay}
            <li
                class="message-date-divider"
                role="separator"
                aria-label={dayLabel}
                data-message-day-divider={dayKey}
            >
                <time class="message-date-chip" datetime={dayKey}>
                    {dayLabel}
                </time>
            </li>
        {/if}
        <li
            class:from-user={message.role === 'user'}
            class:has-date-divider={showsDay}
            class:actions-open={messageActions.activeMessageActionId === message.id}
            class:actions-hovered={messageActions.hoveredMessageActionId === message.id}
            class:memory-source-boundary={messageFocusRequest !== null &&
                (message.id === messageFocusRequest.start_message_id ||
                    message.id === messageFocusRequest.end_message_id)}
            class="message-item"
            data-message-id={message.id}
            use:measureMessage={{
                messageId: message.id,
                epoch: chatScroll.measurementEpoch,
                includesDayDivider: showsDay,
            }}
            tabindex="-1"
            aria-setsize={messageCollection.items.length}
            aria-posinset={virtualWindow.start + localIndex + 1}
            onmouseenter={() => messageActions.hover(message.id, desktop)}
            onmouseleave={() => messageActions.unhover(message.id)}
        >
            <span class="message-avatar" aria-hidden="true"
                >{message.role === 'user'
                    ? '나'
                    : (appState.selected_character?.name ?? '?').slice(0, 1)}</span
            >
            <p class="message-role">
                {message.role === 'user'
                    ? '나'
                    : message.role === 'assistant'
                      ? (appState.selected_character?.name ?? '캐릭터')
                      : '시스템'}
            </p>
            {#if messageActions.editingMessageId === message.id}
                <article class="message-body" aria-label={turnLabel}>
                    <form
                        class="inline-editor"
                        aria-label="메시지 편집"
                        onsubmit={(event) => {
                            event.preventDefault();
                            void commitEdit(message.id);
                        }}
                    >
                        <label class="sr-only" for={messageEditorTextareaControlId}
                            >편집할 메시지</label
                        >
                        <textarea
                            id={messageEditorTextareaControlId}
                            bind:value={messageActions.editDraft}
                            rows="3"></textarea>
                        <div>
                            <button type="button" onclick={() => messageActions.cancelEdit()}>
                                취소
                            </button>
                            <button
                                class="primary"
                                type="submit"
                                disabled={messageActions.editDraft.trim().length === 0}
                            >
                                새 분기로 저장
                            </button>
                        </div>
                    </form>
                    <time class="message-time" datetime={message.created_at}>
                        {formatMessageTime(message.created_at)}
                    </time>
                </article>
            {:else}
                <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
                <article
                    class="message-body"
                    aria-label={turnLabel}
                    tabindex="0"
                    onfocus={() => messageActions.activate(message.id)}
                >
                    <PortableMessage
                        text={runtime.displayText(message)}
                        {client}
                        profile={runtime.profile}
                        enabled={runtime.displayApproved &&
                            runtime.canReadChat &&
                            message.role === 'assistant'}
                        variables={runtime.variables}
                        backgroundMarkup={runtime.background}
                        lastCharacterMessage={runtime.lastCharacterMessage}
                        messageIndex={runtime.canReadChat ? globalIndex : undefined}
                        lastMessageId={runtime.canReadChat
                            ? messageCollection.items.length - 1
                            : undefined}
                        onAction={(action: string) => void runtime.handleAction(action)}
                    />
                    {#if message.status !== 'complete'}
                        <span class="message-status">{message.status}</span>
                    {/if}
                    <footer class="message-bubble-meta">
                        <time class="message-time" datetime={message.created_at}>
                            {formatMessageTime(message.created_at)}
                        </time>
                    </footer>
                </article>
                <ChatMessageActions
                    {message}
                    state={messageActions}
                    generationActive={appState.chat.active_generation_id !== null}
                    onCopy={copyMessage}
                    onCreateBranch={(messageId: string) => void controller.createBranch(messageId)}
                    onRegenerate={(messageId: string) =>
                        void controller.regenerateAssistantMessage(messageId)}
                    {onRemove}
                />
            {/if}
        </li>
    {/each}
    {#if hasLiveResponse}
        <li
            class="message-item streaming-message"
            style:margin-top={`${String(virtualWindow.bottomSpacer)}px`}
        >
            <span class="message-avatar" aria-hidden="true"
                >{(appState.selected_character?.name ?? '?').slice(0, 1)}</span
            >
            <p class="message-role">
                {appState.selected_character?.name ?? '캐릭터'}
            </p>
            <article class="message-body streaming" aria-label="생성 중인 응답">
                {#if appState.chat.reasoning_text !== ''}
                    <details class="stream-reasoning" open>
                        <summary>추론 과정</summary>
                        <p>{appState.chat.reasoning_text}</p>
                    </details>
                {/if}
                {#if appState.chat.streaming_text !== ''}
                    <section class="stream-answer" aria-label="생성 중인 답변">
                        <PortableMessage
                            text={appState.chat.streaming_text}
                            {client}
                            profile={runtime.profile}
                            enabled={runtime.displayApproved && runtime.canReadChat}
                            variables={runtime.variables}
                            backgroundMarkup={runtime.background}
                            lastCharacterMessage={runtime.lastCharacterMessage}
                            messageIndex={runtime.canReadChat
                                ? messageCollection.items.length
                                : undefined}
                            lastMessageId={runtime.canReadChat
                                ? messageCollection.items.length
                                : undefined}
                        />
                    </section>
                {/if}
                <span class="stream-caret" aria-hidden="true"></span>
            </article>
        </li>
    {/if}
</ol>

<style>
    .stream-reasoning {
        color: var(--ink-muted);
    }

    .stream-reasoning summary {
        cursor: pointer;
        font-size: 0.75rem;
        font-weight: 700;
    }

    .stream-reasoning p {
        margin-top: 8px;
        font-size: 0.85rem;
    }

    .stream-answer {
        margin: 0;
    }

    .stream-reasoning + .stream-answer {
        margin-top: 10px;
        padding-top: 10px;
        border-top: 1px solid var(--line);
    }
</style>
