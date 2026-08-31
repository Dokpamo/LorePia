<script lang="ts">
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import type { MemoryRecordSourceNavigationDto } from '../../lib/ipc/contracts';
    import MemoryQueryRetryPanel from '../orchestration/MemoryQueryRetryPanel.svelte';
    import GenerationAttemptApprovals from './GenerationAttemptApprovals.svelte';
    import ChatMessageList from './ChatMessageList.svelte';
    import InteractionRoomSurface from './InteractionRoomSurface.svelte';
    import PortableMessage from './PortableMessage.svelte';
    import type { ChatScrollLifecycle, MessageCollectionSnapshot } from './chat-scroll.svelte';
    import type { InteractionRoomCapableClient } from './interaction-room-controller';
    import type { InteractionRoomLifecycle } from './interaction-room-lifecycle.svelte';
    import type { MessageActionsState } from './message-actions.svelte';
    import type { PortableRuntimeLifecycle } from './portable-runtime-lifecycle.svelte';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        conversationId: string;
        desktop: boolean;
        client?: InteractionRoomCapableClient;
        messageFocusRequest?: (MemoryRecordSourceNavigationDto & { request_id: number }) | null;
        messageCollection: MessageCollectionSnapshot;
        chatScroll: ChatScrollLifecycle;
        messageActions: MessageActionsState;
        runtime: PortableRuntimeLifecycle;
        interactionRoom: InteractionRoomLifecycle;
        attemptApprovalRefreshEpoch: number;
        liveResponseAnnouncement: string;
        chatStatusText: string;
        onNotice: (message: string) => void;
        onRetryGenerationAttempt: (generationAttemptId: string) => void;
        onRemoveMessage: (messageId: string) => Promise<void>;
    }

    let {
        appState,
        controller,
        conversationId,
        desktop,
        client,
        messageFocusRequest = null,
        messageCollection,
        chatScroll,
        messageActions,
        runtime,
        interactionRoom,
        attemptApprovalRefreshEpoch,
        liveResponseAnnouncement,
        chatStatusText,
        onNotice,
        onRetryGenerationAttempt,
        onRemoveMessage,
    }: Props = $props();

    const hasLiveResponse = $derived(
        appState.chat.live_assistant_message_id !== null ||
            appState.chat.streaming_text !== '' ||
            appState.chat.reasoning_text !== '',
    );
    const virtualWindow = $derived.by(() => chatScroll.virtualWindow());

    function syncMessageScrollbarInset(node: HTMLDivElement): { destroy: () => void } {
        const chatPane = node.closest<HTMLElement>('.chat-pane');
        const update = (): void => {
            const scrollbarWidth = Math.max(0, node.offsetWidth - node.clientWidth);
            chatPane?.style.setProperty('--message-scrollbar-width', `${String(scrollbarWidth)}px`);
        };
        window.addEventListener('resize', update);
        update();

        return {
            destroy(): void {
                window.removeEventListener('resize', update);
                chatPane?.style.removeProperty('--message-scrollbar-width');
            },
        };
    }
</script>

{#if client !== undefined && runtime.profile !== null && runtime.background !== ''}
    <div class="portable-runtime-background" aria-hidden="true">
        <PortableMessage
            text={runtime.background}
            {client}
            profile={runtime.profile}
            variables={runtime.variables}
            backgroundMarkup={runtime.background}
            lastCharacterMessage={runtime.lastCharacterMessage}
            messageIndex={runtime.canReadChat ? messageCollection.items.length : undefined}
            lastMessageId={runtime.canReadChat
                ? Math.max(0, messageCollection.items.length - 1)
                : undefined}
        />
    </div>
{/if}

{#if client !== undefined && interactionRoom.controller !== null && interactionRoom.state.phase !== 'unavailable'}
    <InteractionRoomSurface
        {client}
        controller={interactionRoom.controller}
        state={interactionRoom.state}
    />
{/if}

<div
    class="message-scroll"
    use:syncMessageScrollbarInset
    role="region"
    aria-label="메시지 기록"
    tabindex="-1"
    style:scroll-behavior="auto"
    bind:this={chatScroll.scroller}
    onpointerdown={(event) => messageActions.handleMessageScrollPointerDown(event)}
    onscroll={(event) => chatScroll.handleScroll(event)}
>
    {#if appState.messages.phase === 'loading'}
        <div class="state-panel" role="status">메시지를 불러오는 중입니다.</div>
    {:else if appState.messages.phase === 'error'}
        <div class="state-panel error" role="alert">{appState.messages.error}</div>
    {:else if messageCollection.items.length === 0 && !hasLiveResponse}
        <div class="state-panel empty">
            <strong>새로운 이야기의 첫 문장을 보내보세요.</strong>
        </div>
    {:else}
        <ChatMessageList
            {appState}
            {controller}
            {desktop}
            {client}
            {messageFocusRequest}
            {messageCollection}
            {virtualWindow}
            {chatScroll}
            {messageActions}
            {runtime}
            {onNotice}
            onRemove={onRemoveMessage}
        />
    {/if}
</div>

{#if client !== undefined}
    <GenerationAttemptApprovals
        {client}
        {conversationId}
        sourceBranchId={appState.conversation_state?.active_branch_id ?? null}
        headingId="chat-generation-attempt-approvals-title"
        refreshEpoch={attemptApprovalRefreshEpoch}
        onRetry={onRetryGenerationAttempt}
        retryLabel="원래 전송·수정·재생성 확인"
        hideWhenInactive
    />
{/if}

<div class="memory-query-retry-slot">
    <MemoryQueryRetryPanel
        state={appState.memory_query_retries}
        {controller}
        headingId="chat-memory-query-retry-title"
    />
</div>

<div class="sr-only" aria-label="응답 생성 상태" aria-live="polite" aria-atomic="true">
    {liveResponseAnnouncement}
</div>

{#if chatStatusText !== ''}
    <div class="chat-live-status" aria-live="polite" aria-atomic="true">
        {chatStatusText}
    </div>
{/if}

<style>
    .portable-runtime-background {
        position: absolute;
        z-index: 20;
        inset: 0;
        overflow: visible;
        pointer-events: none;
    }

    .memory-query-retry-slot {
        width: min(100% - 2 * clamp(16px, 5vw, 32px), var(--reading));
        margin: 8px auto 0;
    }
</style>
