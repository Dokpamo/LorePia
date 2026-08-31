<script lang="ts">
    import { ArrowLeft, ChevronDown, MessagesSquare, Share2, Sparkles, X } from '@lucide/svelte';
    import PortableMessage from './PortableMessage.svelte';
    import { onMount, tick } from 'svelte';

    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import ChoicePopover from '../../components/ChoicePopover.svelte';
    import SegmentedControl from '../../components/SegmentedControl.svelte';
    import type {
        ConversationMode,
        GenerationSelectionInput,
        MemoryRecordSourceNavigationDto,
        MessageDto,
    } from '../../lib/ipc/contracts';
    import { t } from '../../lib/i18n';
    import MemoryQueryRetryPanel from '../orchestration/MemoryQueryRetryPanel.svelte';
    import type {
        OrchestrationController,
        OrchestrationState,
    } from '../orchestration/orchestration-controller';
    import ChatComposer from './ChatComposer.svelte';
    import ChatFullscreenComposer from './ChatFullscreenComposer.svelte';
    import ChatMessageActions from './ChatMessageActions.svelte';
    import ChatUtilityDrawer from './ChatUtilityDrawer.svelte';
    import {
        ChatScrollLifecycle,
        type MessageCollectionSnapshot,
        type MessageMeasurementInput,
    } from './chat-scroll.svelte';
    import { ChatComposerState } from './composer-state.svelte';
    import GenerationAttemptApprovals from './GenerationAttemptApprovals.svelte';
    import InteractionRoomSurface from './InteractionRoomSurface.svelte';
    import PortableRuntimeControls from './PortableRuntimeControls.svelte';
    import { PortableRuntimeLifecycle } from './portable-runtime-lifecycle.svelte';
    import {
        INITIAL_INTERACTION_ROOM_STATE,
        InteractionRoomController,
        type InteractionRoomCapableClient,
        type InteractionRoomState,
    } from './interaction-room-controller';
    import { MessageActionsState } from './message-actions.svelte';
    import { UtilitySwipeLifecycle } from './utility-swipe.svelte';
    import { VIRTUAL_MESSAGE_BLOCK_PADDING } from './virtual-window';

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
    let interactionController = $state<InteractionRoomController | null>(null);
    const portableRuntimeLifecycle = new PortableRuntimeLifecycle({
        currentMessages: () => appState.messages.items,
        primarySelection: () => controller.runtimeGenerationSelection(),
        onNotice: (message) => {
            copyNotice = message;
        },
    });
    const characterRenderProfile = $derived(portableRuntimeLifecycle.profile);
    const portableRuntime = $derived(portableRuntimeLifecycle.runtime);
    const portableRuntimeModelCall = $derived(portableRuntimeLifecycle.modelCall);
    const portableRuntimePersistenceStatus = $derived(portableRuntimeLifecycle.persistenceStatus);
    const portableRuntimePhase = $derived(portableRuntimeLifecycle.phase);
    const portableRuntimeError = $derived(portableRuntimeLifecycle.error);
    const portableRuntimeRevision = $derived(portableRuntimeLifecycle.revision);
    let dismissedChatError = $state<string | null>(null);
    let dismissedInteractionError = $state<string | null>(null);
    let interactionState = $state<InteractionRoomState>(
        structuredClone(INITIAL_INTERACTION_ROOM_STATE),
    );
    let interactionRoomKey = '';
    let attemptApprovalRefreshEpoch = $state(0);
    let cachedDisplayItemsSource: MessageDto[] | null = null;
    let cachedLiveAssistantMessageId: string | null = null;
    let cachedDisplayItems: MessageDto[] = [];
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

    function projectDisplayMessages(
        items: MessageDto[],
        liveAssistantMessageId: string | null,
    ): MessageDto[] {
        if (
            cachedDisplayItemsSource === items &&
            cachedLiveAssistantMessageId === liveAssistantMessageId
        ) {
            return cachedDisplayItems;
        }
        cachedDisplayItemsSource = items;
        cachedLiveAssistantMessageId = liveAssistantMessageId;
        cachedDisplayItems =
            liveAssistantMessageId === null
                ? items
                : items.filter((message) => message.id !== liveAssistantMessageId);
        return cachedDisplayItems;
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
            interactionController !== null &&
            interactionState.phase !== 'unavailable' &&
            interactionState.error !== dismissedInteractionError
            ? interactionState.error
            : null,
    );
    const visiblePortableRuntimeError = $derived(
        portableRuntimeError !== visibleChatError &&
            portableRuntimeError !== visibleInteractionError
            ? portableRuntimeError
            : null,
    );
    const hasChatErrorNotices = $derived(
        visibleChatError !== null ||
            visiblePortableRuntimeError !== null ||
            visibleInteractionError !== null,
    );
    const visibleCopyNotice = $derived(
        copyNotice !== appState.chat.error &&
            copyNotice !== portableRuntimeError &&
            copyNotice !== interactionState.error
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
        projectDisplayMessages(appState.messages.items, appState.chat.live_assistant_message_id),
    );
    const activePortableRuntimeGrant = $derived(portableRuntimeLifecycle.activeGrant);
    const portableRuntimeVariables = $derived.by(() => {
        void portableRuntimeRevision;
        if (portableRuntime !== null) return portableRuntime.variables;
        if (!(activePortableRuntimeGrant?.capabilities.includes('profile:read') ?? false))
            return {};
        return characterRenderProfile?.initial_variables ?? {};
    });
    const portableDisplayApproved = $derived(
        activePortableRuntimeGrant?.capabilities.includes('ui:write') ?? false,
    );
    const portableRuntimeCanReadChat = $derived(
        activePortableRuntimeGrant?.capabilities.includes('chat:read') ?? false,
    );
    const portableRuntimeBackground = $derived.by(() => {
        void portableRuntimeRevision;
        if (!portableDisplayApproved) return '';
        return portableRuntime?.backgroundMarkup ?? characterRenderProfile?.background_markup ?? '';
    });
    const portableRuntimeCapabilities = $derived(portableRuntimeLifecycle.capabilities);
    const portableRuntimeLastCharacterMessage = $derived.by(() => {
        void portableRuntimeRevision;
        if (!portableRuntimeCanReadChat) return '';
        const message = [...displayMessageItems]
            .reverse()
            .find((candidate) => candidate.role === 'assistant');
        return message === undefined
            ? ''
            : (portableRuntime?.effectiveText(message) ?? message.content);
    });
    const auxiliaryRuntimeModelOptions = $derived.by(() => {
        const options: {
            value: string;
            label: string;
            selection: GenerationSelectionInput | null;
        }[] = [{ value: '', label: '현재 기본 생성 모델', selection: null }];
        for (const preset of appState.providers.workspace.presets) {
            const route = appState.providers.workspace.routes.find(
                (candidate) => candidate.id === preset.model_route_id,
            );
            options.push({
                value: `target:${preset.id}`,
                label: `${route?.display_name ?? route?.model_id ?? preset.model_route_id} · ${preset.display_name}`,
                selection: {
                    kind: 'target',
                    target: {
                        model_route_id: preset.model_route_id,
                        generation_preset_id: preset.id,
                    },
                },
            });
        }
        for (const profile of appState.providers.workspace.legacy_profiles) {
            options.push({
                value: `legacy:${profile.id}`,
                label: `${profile.display_name} · ${profile.model}`,
                selection: { kind: 'legacy_profile', provider_profile_id: profile.id },
            });
        }
        return options;
    });
    const selectedAuxiliaryRuntimeModel = $derived.by(() => {
        void portableRuntimeRevision;
        const selection = portableRuntime?.auxiliarySelection;
        if (selection === null || selection === undefined) return '';
        return selection.kind === 'target'
            ? `target:${selection.target.generation_preset_id}`
            : `legacy:${selection.provider_profile_id}`;
    });
    const portableRuntimeModelBudget = $derived.by(() => {
        void portableRuntimeRevision;
        return portableRuntime?.modelBudget ?? null;
    });
    const messageCollection: MessageCollectionSnapshot = $derived(
        chatScroll.snapshotMessageCollection(displayMessageItems),
    );
    const virtualWindow = $derived.by(() => chatScroll.virtualWindow());
    const visibleMessages = $derived(
        messageCollection.items.slice(virtualWindow.start, virtualWindow.end),
    );

    $effect(() => {
        messageActions.syncCollection(messageCollection);
    });

    const KOREAN_WEEKDAYS = ['일요일', '월요일', '화요일', '수요일', '목요일', '금요일', '토요일'];

    function parsedMessageDate(value: string): Date | null {
        const date = new Date(value);
        return Number.isNaN(date.getTime()) ? null : date;
    }

    function messageDayKey(value: string): string {
        const date = parsedMessageDate(value);
        if (date === null) return value.slice(0, 10);
        return [
            String(date.getFullYear()),
            String(date.getMonth() + 1).padStart(2, '0'),
            String(date.getDate()).padStart(2, '0'),
        ].join('-');
    }

    function formatMessageDay(value: string): string {
        const date = parsedMessageDate(value);
        if (date === null) return '날짜를 확인할 수 없음';
        return `${String(date.getFullYear())}년 ${String(date.getMonth() + 1)}월 ${String(
            date.getDate(),
        )}일 ${KOREAN_WEEKDAYS[date.getDay()] ?? ''}`;
    }

    function formatMessageTime(value: string): string {
        const date = parsedMessageDate(value);
        if (date === null) return '--:--';
        return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(
            2,
            '0',
        )}`;
    }

    function startsMessageDay(message: MessageDto, globalIndex: number): boolean {
        if (globalIndex === 0) return true;
        const previous = messageCollection.items[globalIndex - 1];
        return (
            previous === undefined ||
            messageDayKey(previous.created_at) !== messageDayKey(message.created_at)
        );
    }

    const hasLiveResponse = $derived(
        appState.chat.live_assistant_message_id !== null ||
            appState.chat.streaming_text !== '' ||
            appState.chat.reasoning_text !== '',
    );
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
        if (interactionState.error === null) dismissedInteractionError = null;
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
        const activeController = interactionController;
        const conversationId = appState.selected_conversation?.id ?? null;
        const branchId = appState.conversation_state?.active_branch_id ?? null;
        const nextKey = conversationId && branchId ? `${conversationId}:${branchId}` : '';
        if (activeController === null || nextKey === interactionRoomKey) return;
        interactionRoomKey = nextKey;
        void activeController.loadRoom(conversationId, branchId);
    });

    onMount(() => {
        if (client === undefined) return;
        const roomController = new InteractionRoomController(client);
        interactionController = roomController;
        const unsubscribe = roomController.state.subscribe((value) => {
            interactionState = value;
        });
        return () => {
            unsubscribe();
            roomController.destroy();
            interactionController = null;
        };
    });

    onMount(() => chatScroll.observeScroller());

    function measureMessage(node: HTMLElement, input: MessageMeasurementInput) {
        return chatScroll.measureMessage(node, input);
    }

    function handleScroll(event: Event): void {
        chatScroll.handleScroll(event);
    }

    function handleMessageScrollPointerDown(event: PointerEvent): void {
        messageActions.handleMessageScrollPointerDown(event);
    }

    function activateMessageActions(messageId: string): void {
        messageActions.activate(messageId);
    }

    function hoverMessageActions(messageId: string): void {
        messageActions.hover(messageId, desktop);
    }

    function unhoverMessageActions(messageId: string): void {
        messageActions.unhover(messageId);
    }

    function portableMessageText(message: MessageDto): string {
        void portableRuntimeRevision;
        return portableRuntime?.displayText(message) ?? message.content;
    }

    function portableOptionValue(key: string): string {
        void portableRuntimeRevision;
        return portableRuntime?.optionValue(key) ?? '';
    }

    async function approvePortableRuntime(): Promise<void> {
        await portableRuntimeLifecycle.approve();
    }

    function revokePortableRuntime(): void {
        portableRuntimeLifecycle.revoke();
    }

    async function cancelPortableRuntimeModelCall(): Promise<void> {
        const cancellation = await portableRuntimeLifecycle.cancelActiveModelCall();
        copyNotice =
            cancellation === 'unconfirmed'
                ? t('chat.runtime.cancel.unconfirmed')
                : cancellation === 'confirmed'
                  ? '캐릭터 모델 호출 중지를 요청했습니다.'
                  : '중지할 캐릭터 모델 호출이 없습니다.';
    }

    async function setPortableRuntimeOption(key: string, value: string): Promise<void> {
        await portableRuntimeLifecycle.setOption(key, value);
    }

    function setAuxiliaryRuntimeModel(value: string): void {
        const option = auxiliaryRuntimeModelOptions.find((candidate) => candidate.value === value);
        portableRuntimeLifecycle.setAuxiliarySelection(option?.selection ?? null);
    }

    async function handlePortableRuntimeAction(action: string): Promise<void> {
        await portableRuntimeLifecycle.handleAction(action);
    }

    function dismissPortableRuntimeError(): void {
        const dismissedError = portableRuntimeLifecycle.dismissError();
        if (copyNotice === dismissedError) copyNotice = '';
    }

    async function submit(): Promise<void> {
        const submittedDraft = composer.beginSubmission();
        if (submittedDraft === null) return;
        try {
            let content = submittedDraft;
            let handledByRuntime = false;
            let runtime = portableRuntime;
            if (
                portableRuntimeLifecycle.requiresLuaRuntime &&
                activePortableRuntimeGrant !== null &&
                runtime === null
            ) {
                copyNotice =
                    portableRuntimeError ??
                    '캐릭터 기능을 준비하는 중입니다. 잠시 뒤 다시 보내세요.';
                return;
            }
            const prepared = await portableRuntimeLifecycle.prepareInput(content);
            if (prepared !== null) {
                runtime = prepared.runtime;
                content = prepared.text;
                handledByRuntime = prepared.handledByRuntime;
            }
            const accepted =
                handledByRuntime ||
                (runtime === null
                    ? await controller.sendMessage(content)
                    : await controller.sendMessage(
                          content,
                          portableRuntimeLifecycle.generationVariableOverrides(runtime),
                      ));
            if (accepted) {
                composer.acceptSubmission();
            }
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

    function setMode(mode: ConversationMode): void {
        void controller.setConversationMode(mode);
    }

    async function removeMessageAndResetRuntime(messageId: string): Promise<void> {
        const result = await controller.removeMessage(messageId);
        if (result.mutationCommitted && branchKey === result.scopeKey) {
            portableRuntimeLifecycle.resetScope();
        }
    }

    async function commitEdit(messageId: string): Promise<void> {
        const accepted = await controller.editUserMessage(messageId, messageActions.editDraft);
        if (accepted) messageActions.finishEdit();
    }

    async function copyMessage(message: MessageDto): Promise<void> {
        try {
            await navigator.clipboard.writeText(
                portableRuntime?.effectiveText(message) ?? message.content,
            );
            copyNotice = '메시지를 복사했습니다.';
        } catch {
            copyNotice = '메시지를 복사하지 못했습니다.';
        }
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
                return `${speaker}: ${portableRuntime?.effectiveText(message) ?? message.content}`;
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
    <div class="chat-room-controls">
        <div class="chat-room-control-block">
            <span class="chat-room-control-label">대화 모드</span>
            <SegmentedControl
                id="conversation-mode"
                label="대화 모드"
                value={appState.conversation_state?.selected_mode ?? 'chat'}
                options={[
                    { value: 'chat', label: '채팅' },
                    { value: 'story', label: '스토리' },
                ]}
                onSelect={(value: string) => setMode(value as ConversationMode)}
            />
        </div>

        {#if characterRenderProfile !== null && (characterRenderProfile.runtime_scripts.length > 0 || characterRenderProfile.output_transforms.length > 0 || characterRenderProfile.display_transforms.length > 0 || characterRenderProfile.background_markup.trim().length > 0)}
            <PortableRuntimeControls
                phase={portableRuntimePhase}
                grant={activePortableRuntimeGrant}
                capabilities={portableRuntimeCapabilities}
                bind:selectedCapabilities={portableRuntimeLifecycle.selectedCapabilities}
                runtime={portableRuntime}
                selectedAuxiliaryModel={selectedAuxiliaryRuntimeModel}
                auxiliaryModelOptions={auxiliaryRuntimeModelOptions}
                modelBudget={portableRuntimeModelBudget}
                modelCall={portableRuntimeModelCall}
                persistenceStatus={portableRuntimePersistenceStatus}
                optionValue={portableOptionValue}
                onApprove={approvePortableRuntime}
                onRevoke={revokePortableRuntime}
                onSelectAuxiliaryModel={setAuxiliaryRuntimeModel}
                onSetOption={setPortableRuntimeOption}
                onCancelModelCall={cancelPortableRuntimeModelCall}
            />
        {/if}

        {#if appState.branches.length > 1}
            <div class="branch-picker chat-room-branch">
                <span>분기</span>
                <ChoicePopover
                    id="chat-active-branch"
                    label="분기"
                    value={appState.conversation_state?.active_branch_id ?? ''}
                    showLabel={false}
                    options={appState.branches.map((branch, index) => ({
                        value: branch.id,
                        label: branch.title ?? `분기 ${String(index + 1)}`,
                    }))}
                    onSelect={(value: string) => void controller.selectBranch(value)}
                />
            </div>
        {/if}

        <button
            class="chat-room-new-operation"
            type="button"
            aria-label="새 생성 작업"
            disabled={composer.sending ||
                appState.chat.phase === 'loading' ||
                appState.chat.active_generation_id !== null}
            onclick={async () => {
                controller.beginNewGenerationOperation();
                copyNotice =
                    '새 생성 작업으로 전환했습니다. 같은 입력도 새로운 요청으로 처리됩니다.';
                await closeSettings();
                await tick();
                composer.focusTextarea();
            }}
        >
            <Sparkles aria-hidden="true" />
            <span>
                <strong>새 생성 작업</strong>
                <small>현재 입력을 별도의 새 요청으로 처리합니다.</small>
            </span>
        </button>
    </div>
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
            <div class="chat-error-region" role="region" aria-label="채팅 오류 알림">
                {#if visibleChatError !== null}
                    <div class="chat-error-notice" role="alert">
                        <p>{visibleChatError}</p>
                        <button
                            class="chat-error-dismiss"
                            type="button"
                            aria-label="채팅 오류 닫기"
                            title="닫기"
                            onclick={() => (dismissedChatError = visibleChatError)}
                        >
                            <X aria-hidden="true" />
                        </button>
                    </div>
                {/if}
                {#if visiblePortableRuntimeError !== null}
                    <div class="chat-error-notice" role="alert">
                        <p>{visiblePortableRuntimeError}</p>
                        <button
                            class="chat-error-dismiss"
                            type="button"
                            aria-label="캐릭터 기능 오류 닫기"
                            title="닫기"
                            onclick={dismissPortableRuntimeError}
                        >
                            <X aria-hidden="true" />
                        </button>
                    </div>
                {/if}
                {#if visibleInteractionError !== null}
                    <div class="chat-error-notice" role="alert">
                        <p>{visibleInteractionError}</p>
                        <button
                            class="chat-error-dismiss"
                            type="button"
                            aria-label="대화 상호작용 오류 닫기"
                            title="닫기"
                            onclick={() => (dismissedInteractionError = visibleInteractionError)}
                        >
                            <X aria-hidden="true" />
                        </button>
                    </div>
                {/if}
            </div>
        {/if}

        {#if client !== undefined && characterRenderProfile !== null && portableRuntimeBackground !== ''}
            <div class="portable-runtime-background" aria-hidden="true">
                <PortableMessage
                    text={portableRuntimeBackground}
                    {client}
                    profile={characterRenderProfile}
                    variables={portableRuntimeVariables}
                    backgroundMarkup={portableRuntimeBackground}
                    lastCharacterMessage={portableRuntimeLastCharacterMessage}
                    messageIndex={portableRuntimeCanReadChat
                        ? messageCollection.items.length
                        : undefined}
                    lastMessageId={portableRuntimeCanReadChat
                        ? Math.max(0, messageCollection.items.length - 1)
                        : undefined}
                />
            </div>
        {/if}

        {#if client !== undefined && interactionController !== null && interactionState.phase !== 'unavailable'}
            <InteractionRoomSurface
                {client}
                controller={interactionController}
                state={interactionState}
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
            onpointerdown={handleMessageScrollPointerDown}
            onscroll={handleScroll}
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
                <ol
                    class="message-list virtualized"
                    aria-label="대화 메시지"
                    style:padding-top={String(
                        VIRTUAL_MESSAGE_BLOCK_PADDING + virtualWindow.topSpacer,
                    ) + 'px'}
                    style:padding-bottom={String(
                        VIRTUAL_MESSAGE_BLOCK_PADDING +
                            (hasLiveResponse ? 0 : virtualWindow.bottomSpacer),
                    ) + 'px'}
                >
                    {#each visibleMessages as message, localIndex (message.id)}
                        {@const globalIndex = virtualWindow.start + localIndex}
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
                            class:actions-hovered={messageActions.hoveredMessageActionId ===
                                message.id}
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
                            onmouseenter={() => hoverMessageActions(message.id)}
                            onmouseleave={() => unhoverMessageActions(message.id)}
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
                                        <label class="sr-only" for={`edit-${message.id}`}
                                            >편집할 메시지</label
                                        >
                                        <textarea
                                            id={`edit-${message.id}`}
                                            bind:value={messageActions.editDraft}
                                            rows="3"></textarea>
                                        <div>
                                            <button
                                                type="button"
                                                onclick={() => {
                                                    messageActions.cancelEdit();
                                                }}
                                            >
                                                취소
                                            </button>
                                            <button
                                                class="primary"
                                                type="submit"
                                                disabled={messageActions.editDraft.trim().length ===
                                                    0}
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
                                    onfocus={() => activateMessageActions(message.id)}
                                >
                                    <PortableMessage
                                        text={portableMessageText(message)}
                                        {client}
                                        profile={characterRenderProfile}
                                        enabled={portableDisplayApproved &&
                                            portableRuntimeCanReadChat &&
                                            message.role === 'assistant'}
                                        variables={portableRuntimeVariables}
                                        backgroundMarkup={portableRuntimeBackground}
                                        lastCharacterMessage={portableRuntimeLastCharacterMessage}
                                        messageIndex={portableRuntimeCanReadChat
                                            ? globalIndex
                                            : undefined}
                                        lastMessageId={portableRuntimeCanReadChat
                                            ? messageCollection.items.length - 1
                                            : undefined}
                                        onAction={(action: string) =>
                                            void handlePortableRuntimeAction(action)}
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
                                    onCreateBranch={(messageId: string) =>
                                        void controller.createBranch(messageId)}
                                    onRegenerate={(messageId: string) =>
                                        void controller.regenerateAssistantMessage(messageId)}
                                    onRemove={removeMessageAndResetRuntime}
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
                                            profile={characterRenderProfile}
                                            enabled={portableDisplayApproved &&
                                                portableRuntimeCanReadChat}
                                            variables={portableRuntimeVariables}
                                            backgroundMarkup={portableRuntimeBackground}
                                            lastCharacterMessage={portableRuntimeLastCharacterMessage}
                                            messageIndex={portableRuntimeCanReadChat
                                                ? messageCollection.items.length
                                                : undefined}
                                            lastMessageId={portableRuntimeCanReadChat
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
            {/if}
        </div>

        {#if client !== undefined}
            <GenerationAttemptApprovals
                {client}
                conversationId={appState.selected_conversation.id}
                sourceBranchId={appState.conversation_state?.active_branch_id ?? null}
                headingId="chat-generation-attempt-approvals-title"
                refreshEpoch={attemptApprovalRefreshEpoch}
                onRetry={returnToRetainedGenerationInput}
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

<style>
    .chat-room-controls,
    .chat-room-control-block {
        display: grid;
        gap: 8px;
    }

    .chat-room-control-label,
    .chat-room-branch > span {
        color: var(--ink-muted);
        font-size: 0.75rem;
        font-weight: 650;
    }

    .chat-room-branch {
        min-height: 42px;
        justify-content: space-between;
        padding: 0 4px;
    }

    .chat-room-branch :global(.choice-popover) {
        width: min(68%, 240px);
    }

    .portable-runtime-background {
        position: absolute;
        z-index: 20;
        inset: 0;
        overflow: visible;
        pointer-events: none;
    }

    .chat-error-region {
        display: grid;
        position: absolute;
        z-index: 24;
        top: calc(
            env(safe-area-inset-top) + var(--mobile-top-offset) + var(--mobile-top-action) + 10px
        );
        right: 0;
        left: 0;
        justify-items: center;
        padding: 0 var(--chat-side-inset);
        gap: 8px;
        pointer-events: none;
    }

    .chat-error-notice {
        display: grid;
        width: min(100%, 680px);
        min-height: 38px;
        grid-template-columns: minmax(0, 1fr) auto;
        align-items: center;
        padding: 7px 7px 7px 12px;
        border: 1px solid var(--status-error-border);
        border-radius: 11px;
        background: var(--status-error-bg);
        box-shadow: var(--popover-shadow);
        color: var(--status-error-fg);
        font-size: 0.75rem;
        line-height: 1.45;
        animation: chat-error-notice-enter 180ms var(--panel-open-easing) both;
        gap: 10px;
        pointer-events: auto;
    }

    .chat-error-notice p {
        overflow-wrap: anywhere;
        margin: 0;
    }

    .chat-error-dismiss {
        display: grid;
        width: 28px;
        height: 28px;
        min-width: 28px;
        min-height: 28px;
        padding: 0;
        border: 0;
        border-radius: var(--radius-sm);
        background: transparent;
        color: currentcolor;
        place-items: center;
    }

    .chat-error-dismiss :global(svg) {
        width: 15px;
        height: 15px;
        stroke-width: 1.9;
    }

    :global(.app-shell[data-layout='desktop']) .chat-error-region {
        top: 72px;
        padding-inline: var(--chat-side-inset);
        transition:
            right var(--panel-close-duration) var(--panel-close-easing),
            padding-inline var(--panel-close-duration) var(--panel-close-easing);
    }

    :global(.app-shell[data-layout='desktop']) .chat-pane.utility-open > .chat-error-region {
        right: var(--chat-utility-reserved-width);
        padding-inline: var(--chat-utility-side-inset);
        transition:
            right var(--panel-open-duration) var(--panel-open-easing),
            padding-inline var(--panel-open-duration) var(--panel-open-easing);
    }

    @keyframes chat-error-notice-enter {
        from {
            opacity: 0;
            transform: translate3d(0, -6px, 0) scale(0.99);
        }

        to {
            opacity: 1;
            transform: translate3d(0, 0, 0) scale(1);
        }
    }

    .chat-room-new-operation {
        display: flex;
        width: 100%;
        min-height: 54px;
        align-items: center;
        padding: 9px 12px;
        border: 0;
        border-radius: var(--radius-md);
        background: var(--surface-raised);
        box-shadow: var(--shadow-1);
        color: var(--ink);
        gap: 10px;
        text-align: left;
    }

    .chat-room-new-operation :global(svg) {
        width: 20px;
        height: 20px;
        flex: none;
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 1.8;
    }

    .chat-room-new-operation > span {
        display: grid;
        min-width: 0;
        gap: 2px;
    }

    .chat-room-new-operation strong {
        font-size: 0.875rem;
    }

    .chat-room-new-operation small {
        color: var(--ink-muted);
        font-size: 0.72rem;
        line-height: 1.35;
    }

    .memory-query-retry-slot {
        width: min(100% - 2 * clamp(16px, 5vw, 32px), var(--reading));
        margin: 8px auto 0;
    }

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
