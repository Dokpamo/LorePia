<script lang="ts">
    import { ArrowLeft, ChevronDown, MessagesSquare, Share2, Sparkles } from '@lucide/svelte';
    import { onMount } from 'svelte';

    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import type { MemoryRecordSourceNavigationDto } from '../../lib/ipc/contracts';
    import { t } from '../../lib/i18n';
    import type {
        OrchestrationController,
        OrchestrationState,
    } from '../orchestration/orchestration-controller';
    import ChatComposer from './ChatComposer.svelte';
    import ChatErrorRegion from './ChatErrorRegion.svelte';
    import ChatFullscreenComposer from './ChatFullscreenComposer.svelte';
    import ChatRoomControls from './ChatRoomControls.svelte';
    import ChatUtilityDrawer from './ChatUtilityDrawer.svelte';
    import ChatViewport from './ChatViewport.svelte';
    import {
        ChatScrollLifecycle,
        DisplayMessageProjection,
        messageDayKey,
        type MessageCollectionSnapshot,
    } from './chat-scroll.svelte';
    import { ChatComposerState } from './composer-state.svelte';
    import { InteractionRoomLifecycle } from './interaction-room-lifecycle.svelte';
    import { PortableRuntimeLifecycle } from './portable-runtime-lifecycle.svelte';
    import type { InteractionRoomCapableClient } from './interaction-room-controller';
    import { MessageActionsState } from './message-actions.svelte';
    import { UtilitySwipeLifecycle } from './utility-swipe.svelte';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        desktop?: boolean;
        titlebarOverlay?: boolean;
        utilityOpen?: boolean;
        orchestrationState?: OrchestrationState;
        orchestrationController?: OrchestrationController;
        client?: InteractionRoomCapableClient;
        messageFocusRequest?: (MemoryRecordSourceNavigationDto & { request_id: number }) | null;
        onOpenHome?: () => void;
    }

    let {
        appState,
        controller,
        desktop = false,
        titlebarOverlay = false,
        utilityOpen = $bindable(false),
        orchestrationState,
        orchestrationController,
        client,
        messageFocusRequest = null,
        onOpenHome = () => undefined,
    }: Props = $props();
    let copyNotice = $state('');
    let handledMessageFocusRequestId = 0;
    const interactionRoom = new InteractionRoomLifecycle();
    const portableRuntimeLifecycle = new PortableRuntimeLifecycle({
        currentMessages: () => appState.messages.items,
        displayMessages: () => displayMessageItems,
        providerWorkspace: () => appState.providers.workspace,
        primarySelection: () => controller.runtimeGenerationSelection(),
        onNotice: (message) => {
            copyNotice = message;
        },
    });
    let dismissedChatError = $state<string | null>(null);
    let dismissedInteractionError = $state<string | null>(null);
    let attemptApprovalRefreshEpoch = $state(0);
    const displayMessageProjection = new DisplayMessageProjection();
    let liveResponseAnnouncement = $state('');
    let observedLiveResponsePhase: 'idle' | 'reasoning' | 'answer' = 'idle';
    let observedLiveResponseBranchKey = '';
    let awaitingDurableAssistant = false;
    let observedGenerationId: string | null = null;
    let observedLiveAssistantMessageId: string | null = null;
    let utilityView = $state<'tools' | 'settings'>('tools');
    const chatScroll: ChatScrollLifecycle = new ChatScrollLifecycle({
        currentCollection: (): MessageCollectionSnapshot => messageCollection,
        messageDayKey,
        onMemorySourceMissing: notifyMissingMemorySource,
        onMemorySourceFocused: notifyFocusedMemorySource,
    });
    const messageActions = new MessageActionsState(chatScroll);
    const composer = new ChatComposerState({
        chatScroll,
        currentActiveMessageActionId: () => messageActions.activeMessageActionId,
        currentDesktop: () => desktop,
        onBranchReset: () => {
            utilityView = 'tools';
        },
        onSubmit: submit,
    });
    const utilitySwipe = new UtilitySwipeLifecycle({
        composerFullscreen: () => composer.fullscreen,
        desktop: () => desktop,
        open: () => utilityOpen,
        openTools: () => {
            utilityView = 'tools';
            utilityOpen = true;
        },
    });

    function handleUtilityOpenPointerDown(event: PointerEvent): void {
        utilitySwipe.pointerDown(event);
    }

    function handleUtilityOpenPointerMove(event: PointerEvent): void {
        utilitySwipe.pointerMove(event);
    }

    function handleUtilityOpenPointerUp(event: PointerEvent): void {
        utilitySwipe.pointerUp(event);
    }

    function handleUtilityOpenPointerCancel(event: PointerEvent): void {
        utilitySwipe.pointerCancel(event);
    }

    function handleUtilityOpenClickCapture(event: MouseEvent): void {
        utilitySwipe.clickCapture(event);
    }

    const branchKey = $derived(
        appState.selected_conversation && appState.conversation_state
            ? `${appState.selected_conversation.id}:${appState.conversation_state.active_branch_id}`
            : '',
    );
    const visibleChatError = $derived(
        appState.chat.error !== dismissedChatError ? appState.chat.error : null,
    );
    const visibleInteractionError = $derived(
        client !== undefined &&
            interactionRoom.controller !== null &&
            interactionRoom.state.phase !== 'unavailable' &&
            interactionRoom.state.error !== dismissedInteractionError
            ? interactionRoom.state.error
            : null,
    );
    const visiblePortableRuntimeError = $derived(
        portableRuntimeLifecycle.error !== visibleChatError &&
            portableRuntimeLifecycle.error !== visibleInteractionError
            ? portableRuntimeLifecycle.error
            : null,
    );
    const hasChatErrorNotices = $derived(
        visibleChatError !== null ||
            visiblePortableRuntimeError !== null ||
            visibleInteractionError !== null,
    );
    const visibleCopyNotice = $derived(
        copyNotice !== appState.chat.error &&
            copyNotice !== portableRuntimeLifecycle.error &&
            copyNotice !== interactionRoom.state.error
            ? copyNotice
            : '',
    );
    const chatStatusText = $derived(
        appState.chat.reconcile_notice ?? appState.chat.usage_label ?? visibleCopyNotice,
    );
    const selectedModelLabel = $derived.by(() => {
        const roomPresetId = orchestrationState?.workspace.room_config.generation_preset_id;
        const roomPreset = appState.providers.workspace.presets.find(
            (candidate) => candidate.id === roomPresetId,
        );
        const routeId =
            roomPreset?.model_route_id ??
            appState.providers.workspace.settings.selected_model_route_id;
        if (routeId === null) return '';
        const route = appState.providers.workspace.routes.find(
            (candidate) => candidate.id === routeId,
        );
        return route?.display_name ?? route?.model_id ?? '';
    });
    const reasoningEffortLabel = $derived.by(() => {
        const effort = orchestrationState?.workspace.room_config.reasoning_effort;
        switch (effort) {
            case 'minimal':
                return '최소';
            case 'low':
                return '낮음';
            case 'medium':
                return '중간';
            case 'high':
                return '높음';
            case 'extra_high':
                return '매우 높음';
            case 'maximum':
                return '최대';
            default:
                return '';
        }
    });
    const composerConfigurationLabel = $derived(
        [selectedModelLabel, reasoningEffortLabel].filter((value) => value !== '').join(' · '),
    );
    const displayMessageItems = $derived(
        displayMessageProjection.project(
            appState.messages.items,
            appState.chat.live_assistant_message_id,
        ),
    );
    const messageCollection: MessageCollectionSnapshot = $derived(
        chatScroll.snapshotMessageCollection(displayMessageItems),
    );

    $effect(() => {
        messageActions.syncCollection(messageCollection);
    });

    $effect(() => {
        const nextKey = branchKey;
        composer.syncBranch(nextKey);
    });

    $effect(() => {
        composer.syncDesktop(desktop);
    });

    $effect(() => {
        const nextBranchKey = branchKey;
        const liveAssistantMessageId = appState.chat.live_assistant_message_id;
        const activeGenerationId = appState.chat.active_generation_id;
        const responsePhase =
            appState.chat.streaming_text !== ''
                ? 'answer'
                : appState.chat.reasoning_text !== ''
                  ? 'reasoning'
                  : 'idle';
        const liveResponseVisible = liveAssistantMessageId !== null || responsePhase !== 'idle';
        const messages = appState.messages.items;

        if (nextBranchKey !== observedLiveResponseBranchKey) {
            observedLiveResponseBranchKey = nextBranchKey;
            observedLiveResponsePhase = 'idle';
            awaitingDurableAssistant = false;
            observedGenerationId = null;
            observedLiveAssistantMessageId = null;
            liveResponseAnnouncement = '';
        }

        if (liveResponseVisible) {
            awaitingDurableAssistant = true;
            if (activeGenerationId !== null) observedGenerationId = activeGenerationId;
            if (liveAssistantMessageId !== null) {
                observedLiveAssistantMessageId = liveAssistantMessageId;
            }
            if (responsePhase !== observedLiveResponsePhase) {
                if (responsePhase === 'reasoning') {
                    liveResponseAnnouncement = '응답의 추론을 생성하고 있습니다.';
                } else if (responsePhase === 'answer') {
                    liveResponseAnnouncement = '응답 본문 생성을 시작했습니다.';
                }
            }
            observedLiveResponsePhase = responsePhase;
            return;
        }

        if (awaitingDurableAssistant && activeGenerationId === null) {
            const durableAssistant = messages.find(
                (message) =>
                    message.role === 'assistant' &&
                    (observedLiveAssistantMessageId !== null
                        ? message.id === observedLiveAssistantMessageId
                        : observedGenerationId !== null &&
                          message.generation_id === observedGenerationId),
            );
            if (durableAssistant?.status === 'complete') {
                liveResponseAnnouncement = '응답 생성이 완료됐습니다.';
            }
            awaitingDurableAssistant = false;
            observedGenerationId = null;
            observedLiveAssistantMessageId = null;
        }
        observedLiveResponsePhase = 'idle';
    });

    $effect(() => {
        const nextKey = branchKey;
        chatScroll.syncBranch(nextKey, () => messageActions.resetTransientActions());
    });

    $effect(() => {
        const collection = messageCollection;
        chatScroll.syncCollection(collection);
    });

    $effect(() => {
        const messageCount = messageCollection.items.length;
        const liveResponseLength =
            appState.chat.streaming_text.length + appState.chat.reasoning_text.length;
        chatScroll.syncMessageGrowth(messageCount, liveResponseLength);
    });

    $effect(() => {
        const request = messageFocusRequest;
        if (request === null || request.request_id === handledMessageFocusRequestId) return;
        handledMessageFocusRequestId = request.request_id;
        void chatScroll.focusMemorySource(request);
    });

    function notifyMissingMemorySource(): void {
        copyNotice = '장기기억 출처 메시지가 현재 로드된 대화 기록에 없습니다.';
    }

    function notifyFocusedMemorySource(request: MemoryRecordSourceNavigationDto): void {
        copyNotice =
            request.start_message_id === request.end_message_id
                ? '장기기억 출처 메시지로 이동했습니다.'
                : '장기기억 출처 범위의 첫 메시지로 이동했습니다.';
    }

    $effect(() => {
        if (appState.chat.error === null) dismissedChatError = null;
        if (interactionRoom.state.error === null) dismissedInteractionError = null;
    });

    $effect(() => {
        const characterId = appState.selected_character?.id ?? null;
        return portableRuntimeLifecycle.loadProfile(client, characterId);
    });

    $effect(() => {
        return portableRuntimeLifecycle.recreate({
            client,
            conversationId: appState.selected_conversation?.id ?? null,
            branchId: appState.conversation_state?.active_branch_id ?? null,
            character: appState.selected_character,
        });
    });

    $effect(() => {
        const messages = appState.messages.items;
        portableRuntimeLifecycle.syncMessages({
            messages,
            activeGenerationId: appState.chat.active_generation_id,
            hasStreamingPresentation:
                appState.chat.live_assistant_message_id !== null ||
                appState.chat.streaming_text !== '' ||
                appState.chat.reasoning_text !== '',
        });
    });

    $effect(() => {
        const conversationId = appState.selected_conversation?.id ?? null;
        const branchId = appState.conversation_state?.active_branch_id ?? null;
        interactionRoom.syncRoom(conversationId, branchId);
    });

    onMount(() => interactionRoom.mount(client));
    onMount(() => chatScroll.observeScroller());

    async function submit(): Promise<void> {
        const submittedDraft = composer.beginSubmission();
        if (submittedDraft === null) return;
        try {
            const accepted = await portableRuntimeLifecycle.dispatchInput(
                submittedDraft,
                (content, variableOverrides) =>
                    variableOverrides === undefined
                        ? controller.sendMessage(content)
                        : controller.sendMessage(content, variableOverrides),
            );
            if (accepted === null) return;
            if (accepted) composer.acceptSubmission();
            attemptApprovalRefreshEpoch += 1;
        } catch (error) {
            copyNotice = portableRuntimeLifecycle.fail(error, t('chat.runtime.execution_failed'));
        } finally {
            composer.finishSubmission();
        }
    }

    function returnToRetainedGenerationInput(generationAttemptId: string): void {
        if (!controller.stageGenerationAttemptRetry(generationAttemptId)) {
            copyNotice = '승인된 생성 시도 식별자를 확인하지 못했습니다. 목록을 새로고침하세요.';
            return;
        }
        copyNotice =
            '승인된 생성 시도를 준비했습니다. 원래 전송·수정·재생성 작업을 직접 반복하세요.';
    }

    async function removeMessageAndResetRuntime(messageId: string): Promise<void> {
        const result = await controller.removeMessage(messageId);
        if (result.mutationCommitted && branchKey === result.scopeKey)
            portableRuntimeLifecycle.resetScope();
    }

    function openConversationTools(): void {
        utilityView = 'tools';
        utilityOpen = true;
    }

    function openConversationSettings(): void {
        utilityView = 'settings';
        utilityOpen = true;
    }

    async function shareConversation(): Promise<void> {
        const title = appState.selected_conversation?.title ?? 'LorePia 대화';
        const text = displayMessageItems
            .map((message) => {
                const speaker =
                    message.role === 'user'
                        ? '나'
                        : (appState.selected_character?.name ?? '캐릭터');
                return `${speaker}: ${portableRuntimeLifecycle.effectiveText(message)}`;
            })
            .join('\n\n');
        try {
            if (typeof navigator.share === 'function') {
                await navigator.share({ title, text });
                copyNotice = '대화 공유 창을 열었습니다.';
            } else {
                await navigator.clipboard.writeText(`${title}\n\n${text}`);
                copyNotice = '공유할 대화 내용을 복사했습니다.';
            }
        } catch (error) {
            if (error instanceof DOMException && error.name === 'AbortError') return;
            copyNotice = '대화를 공유하지 못했습니다.';
        }
    }
</script>

{#snippet roomControls(closeSettings: () => Promise<void>)}
    <ChatRoomControls
        {appState}
        {controller}
        {composer}
        runtime={portableRuntimeLifecycle}
        onNotice={(message: string) => (copyNotice = message)}
        {closeSettings}
    />
{/snippet}

<section
    class="pane chat-pane"
    class:desktop-composer={desktop}
    class:utility-open={utilityOpen}
    data-conversation-mode={appState.conversation_state?.selected_mode ?? 'chat'}
    data-utility-open-gesture={utilitySwipe.gesture}
    aria-labelledby="chat-title"
    onpointerdown={handleUtilityOpenPointerDown}
    onpointermove={handleUtilityOpenPointerMove}
    onpointerup={handleUtilityOpenPointerUp}
    onpointercancel={handleUtilityOpenPointerCancel}
    onclickcapture={handleUtilityOpenClickCapture}
>
    {#if appState.selected_conversation === null}
        <header
            class="mobile-top-frame chat-header empty-chat-header"
            data-tauri-drag-region={titlebarOverlay ? '' : undefined}
        >
            <div class="chat-identity" data-tauri-drag-region={titlebarOverlay ? '' : undefined}>
                <h2 id="chat-title" data-tauri-drag-region={titlebarOverlay ? '' : undefined}>
                    채팅
                </h2>
            </div>
        </header>
        <div class="chat-placeholder state-panel empty">
            <span class="large-mark" aria-hidden="true">
                <Sparkles class="chat-placeholder-icon" />
            </span>
            <strong>대화를 선택하세요.</strong>
            <button class="primary" type="button" onclick={onOpenHome}> 대화 목록 열기 </button>
        </div>
    {:else}
        <header
            class="mobile-top-frame mobile-top-frame-leading chat-header"
            data-tauri-drag-region={titlebarOverlay ? '' : undefined}
        >
            <button
                class="icon-button ghost mobile-top-action mobile-top-action-left back-button"
                type="button"
                aria-label="대화 목록으로"
                onclick={onOpenHome}
            >
                <ArrowLeft class="chat-back-icon" aria-hidden="true" />
            </button>
            <div class="chat-identity" data-tauri-drag-region={titlebarOverlay ? '' : undefined}>
                {#if desktop}
                    <button
                        class="chat-title-context"
                        type="button"
                        aria-label={`${appState.selected_conversation.title} 대화 도구 열기`}
                        aria-expanded={utilityOpen && utilityView === 'tools'}
                        onclick={openConversationTools}
                    >
                        <MessagesSquare aria-hidden="true" />
                        <h2 id="chat-title">{appState.selected_conversation.title}</h2>
                        <ChevronDown aria-hidden="true" />
                    </button>
                {:else}
                    <span
                        class="avatar"
                        aria-hidden="true"
                        data-tauri-drag-region={titlebarOverlay ? '' : undefined}
                        >{(appState.selected_character?.name ?? '?').slice(0, 1)}</span
                    >
                    <div data-tauri-drag-region={titlebarOverlay ? '' : undefined}>
                        <h2
                            id="chat-title"
                            data-tauri-drag-region={titlebarOverlay ? '' : undefined}
                        >
                            {appState.selected_conversation.title}
                        </h2>
                        <p
                            class="chat-subtitle"
                            data-tauri-drag-region={titlebarOverlay ? '' : undefined}
                        >
                            {appState.selected_character?.name ?? 'Character'}
                        </p>
                    </div>
                {/if}
            </div>
            <div class="chat-controls">
                {#if desktop}
                    <button
                        class="chat-header-action"
                        type="button"
                        aria-label="대화 공유"
                        title="대화 공유"
                        onclick={() => void shareConversation()}
                    >
                        <Share2 aria-hidden="true" />
                    </button>
                {/if}
                {#if orchestrationState && orchestrationController}
                    <ChatUtilityDrawer
                        {appState}
                        {orchestrationState}
                        {orchestrationController}
                        {desktop}
                        bind:open={utilityOpen}
                        bind:view={utilityView}
                        onOpen={() => {
                            if (appState.providers.phase === 'idle') {
                                void controller.loadProviders();
                            }
                        }}
                        {roomControls}
                    />
                {/if}
            </div>
        </header>

        {#if hasChatErrorNotices}
            <ChatErrorRegion
                chatError={visibleChatError}
                runtimeError={visiblePortableRuntimeError}
                interactionError={visibleInteractionError}
                onDismissChat={() => (dismissedChatError = visibleChatError)}
                onDismissRuntime={() =>
                    (copyNotice = portableRuntimeLifecycle.dismissErrorNotice(copyNotice))}
                onDismissInteraction={() => (dismissedInteractionError = visibleInteractionError)}
            />
        {/if}

        <ChatViewport
            {appState}
            {controller}
            conversationId={appState.selected_conversation.id}
            {desktop}
            {client}
            {messageFocusRequest}
            {messageCollection}
            {chatScroll}
            {messageActions}
            runtime={portableRuntimeLifecycle}
            {interactionRoom}
            {attemptApprovalRefreshEpoch}
            {liveResponseAnnouncement}
            {chatStatusText}
            onNotice={(message: string) => (copyNotice = message)}
            onRetryGenerationAttempt={returnToRetainedGenerationInput}
            onRemoveMessage={removeMessageAndResetRuntime}
        />

        <ChatComposer
            state={composer}
            {desktop}
            {composerConfigurationLabel}
            disabled={appState.chat.phase === 'loading' ||
                appState.chat.active_generation_id !== null ||
                appState.conversation_state === null}
            generationActive={appState.chat.active_generation_id !== null}
            onSubmit={submit}
            onOpenSettings={openConversationSettings}
            onCancelGeneration={() => controller.cancelGeneration()}
        />
        <ChatFullscreenComposer
            state={composer}
            disabled={appState.chat.phase === 'loading' ||
                appState.chat.active_generation_id !== null ||
                appState.conversation_state === null}
            onSubmit={submit}
        />
    {/if}
</section>
