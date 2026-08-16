<script lang="ts">
    import { onMount, tick } from 'svelte';
    import type { KeyboardEventHandler } from 'svelte/elements';
    import { SvelteMap, SvelteSet } from 'svelte/reactivity';

    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import type {
        ConversationMode,
        MemoryRecordSourceNavigationDto,
        MessageDto,
    } from '../../lib/ipc/contracts';
    import TrustedAsset from '../assets/TrustedAsset.svelte';
    import MemoryQueryRetryPanel from '../orchestration/MemoryQueryRetryPanel.svelte';
    import OrchestrationQuickDrawer from '../orchestration/OrchestrationQuickDrawer.svelte';
    import type {
        OrchestrationController,
        OrchestrationState,
    } from '../orchestration/orchestration-controller';
    import { shouldSubmitComposer } from './composer';
    import GenerationAttemptApprovals from './GenerationAttemptApprovals.svelte';
    import {
        INITIAL_INTERACTION_ROOM_STATE,
        InteractionRoomController,
        type InteractionRoomCapableClient,
        type InteractionRoomState,
        type RoomInteractionEffect,
    } from './interaction-room-controller';
    import {
        VIRTUAL_MESSAGE_BLOCK_PADDING,
        VirtualMessageLayoutIndex,
        VirtualMessageMeasurements,
        computeAnchoredScrollTop,
        computeVirtualMessageWindow,
        findRetainedMessagePredecessorIndex,
        isVirtualMessageNearBottom,
        virtualMessageOffset,
        virtualWindowContainsIndex,
    } from './virtual-window';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        orchestrationState?: OrchestrationState;
        orchestrationController?: OrchestrationController;
        client?: InteractionRoomCapableClient;
        messageFocusRequest?: (MemoryRecordSourceNavigationDto & { request_id: number }) | null;
        onOpenOrchestrationStudio?: () => void;
    }

    interface MessageMeasurementInput {
        messageId: string;
        epoch: number;
    }

    interface ScrollAnchorSnapshot {
        messageId: string;
        relativeTop: number;
        scrollTop: number;
        virtualTop: number;
        preservesPreMutationPosition?: boolean;
    }

    interface MessageCollectionSnapshot {
        items: MessageDto[];
        ids: string[];
        retainedIds: ReadonlySet<string>;
        indexesById: Readonly<Record<string, number | undefined>>;
    }

    let {
        appState,
        controller,
        orchestrationState,
        orchestrationController,
        client,
        messageFocusRequest = null,
        onOpenOrchestrationStudio = () => undefined,
    }: Props = $props();
    let draft = $state('');
    let compositionActive = $state(false);
    let sending = $state(false);
    let activeDraftKey = '';
    let messageScroller = $state<HTMLDivElement | null>(null);
    let scrollTop = $state(0);
    let viewportHeight = $state(720);
    let nearBottom = $state(true);
    let scrollAnchorEpoch = 0;
    let anchoredBranchKey = '';
    let anchoredMessageCount = 0;
    let editingMessageId = $state<string | null>(null);
    let editDraft = $state('');
    let pendingRemoveId = $state<string | null>(null);
    let copyNotice = $state('');
    let handledMessageFocusRequestId = 0;
    let interactionController = $state<InteractionRoomController | null>(null);
    let interactionState = $state<InteractionRoomState>(
        structuredClone(INITIAL_INTERACTION_ROOM_STATE),
    );
    let interactionRoomKey = '';
    let attemptApprovalRefreshEpoch = $state(0);
    let composerTextarea = $state<HTMLTextAreaElement | null>(null);
    const messageMeasurements = new VirtualMessageMeasurements();
    const virtualLayout = new VirtualMessageLayoutIndex();
    let measurementEpoch = $state(messageMeasurements.epoch);
    let virtualLayoutRevision = $state(virtualLayout.revision);
    let measurementFlushQueued = false;
    let pendingMeasurementEpoch = 0;
    let messageCollectionEpoch = 0;
    let pendingMessageCollectionEpoch = 0;
    let pendingMeasurementScrollEpoch = 0;
    let pendingMeasurementPinBottom = false;
    let pendingMeasurementAnchor: ScrollAnchorSnapshot | null = null;
    let stableMeasurementAnchor: ScrollAnchorSnapshot | null = null;
    let stableAnchorCaptureEpoch = 0;
    let cachedDisplayItemsSource: MessageDto[] | null = null;
    let cachedLiveAssistantMessageId: string | null = null;
    let cachedDisplayItems: MessageDto[] = [];
    let cachedMessageCollection: MessageCollectionSnapshot | null = null;
    let observedMessageCollection: MessageCollectionSnapshot | null = null;
    let liveResponseAnnouncement = $state('');
    let observedLiveResponsePhase: 'idle' | 'reasoning' | 'answer' = 'idle';
    let observedLiveResponseBranchKey = '';
    let awaitingDurableAssistant = false;
    let observedGenerationId: string | null = null;
    let observedLiveAssistantMessageId: string | null = null;
    const drafts = new SvelteMap<string, string>();

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

    function snapshotMessageCollection(items: MessageDto[]): MessageCollectionSnapshot {
        if (cachedMessageCollection?.items === items) return cachedMessageCollection;
        const ids = new Array<string>(items.length);
        const indexesById = Object.create(null) as Record<string, number>;
        for (let index = 0; index < items.length; index += 1) {
            const messageId = items[index]?.id;
            if (messageId === undefined) continue;
            ids[index] = messageId;
            indexesById[messageId] = index;
        }
        cachedMessageCollection = {
            items,
            ids,
            retainedIds: new Set(ids),
            indexesById,
        };
        return cachedMessageCollection;
    }

    const branchKey = $derived(
        appState.selected_conversation && appState.conversation_state
            ? `${appState.selected_conversation.id}:${appState.conversation_state.active_branch_id}`
            : '',
    );
    const displayMessageItems = $derived(
        projectDisplayMessages(appState.messages.items, appState.chat.live_assistant_message_id),
    );
    const messageCollection = $derived(snapshotMessageCollection(displayMessageItems));
    const virtualLayoutSnapshot = $derived({
        layout: virtualLayout,
        revision: virtualLayoutRevision,
    });
    const virtualWindow = $derived(
        computeVirtualMessageWindow(
            virtualLayoutSnapshot.layout,
            Math.max(0, scrollTop - VIRTUAL_MESSAGE_BLOCK_PADDING),
            viewportHeight,
        ),
    );
    const visibleMessages = $derived(
        messageCollection.items.slice(virtualWindow.start, virtualWindow.end),
    );
    const hasLiveResponse = $derived(
        appState.chat.live_assistant_message_id !== null ||
            appState.chat.streaming_text !== '' ||
            appState.chat.reasoning_text !== '',
    );
    const displayInteractionEffects = $derived.by(() => {
        const retained: RoomInteractionEffect[] = [];
        const visualRegions = new SvelteSet<string>();
        for (const effect of [...interactionState.effects].reverse()) {
            if (
                effect.effect.kind === 'state_changed' ||
                effect.effect.kind === 'knowledge_activated'
            ) {
                continue;
            }
            if (effect.effect.kind === 'show_asset') {
                if (visualRegions.has(effect.effect.region)) continue;
                visualRegions.add(effect.effect.region);
            }
            retained.push(effect);
            if (retained.length >= 32) break;
        }
        return retained.reverse();
    });

    $effect(() => {
        const nextKey = branchKey;
        if (nextKey !== activeDraftKey) {
            if (activeDraftKey !== '') drafts.set(activeDraftKey, draft);
            draft = drafts.get(nextKey) ?? '';
            activeDraftKey = nextKey;
        }
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
        if (nextKey === anchoredBranchKey) return;
        const anchorBeforeInvalidation = nearBottom
            ? null
            : (stableMeasurementAnchor ?? captureScrollAnchor());
        anchoredBranchKey = nextKey;
        measurementEpoch = messageMeasurements.resetScope(nextKey);
        virtualLayoutRevision = virtualLayout.reset(
            messageCollection.ids,
            messageMeasurements.values,
        );
        anchoredMessageCount = messageCollection.items.length;
        editingMessageId = null;
        pendingRemoveId = null;
        const epoch = ++scrollAnchorEpoch;
        if (
            anchorBeforeInvalidation !== null &&
            messageCollection.retainedIds.has(anchorBeforeInvalidation.messageId)
        ) {
            scheduleMeasurementFlush(anchorBeforeInvalidation);
        } else {
            stableMeasurementAnchor = null;
            nearBottom = true;
            void scrollToBottom(epoch);
        }
    });

    $effect(() => {
        const collection = messageCollection;
        const previousCollection = observedMessageCollection;
        const collectionChanged = previousCollection?.items !== collection.items;
        const anchorBeforeCollectionChange = stableMeasurementAnchor ?? pendingMeasurementAnchor;
        const anchorRemoved =
            collectionChanged &&
            anchorBeforeCollectionChange !== null &&
            !collection.retainedIds.has(anchorBeforeCollectionChange.messageId);
        const retainedAnchor = anchorRemoved
            ? deriveRetainedCollectionAnchor(
                  anchorBeforeCollectionChange,
                  previousCollection,
                  collection,
              )
            : anchorBeforeCollectionChange;
        if (collectionChanged) messageCollectionEpoch += 1;
        const pruned = messageMeasurements.prune(collection.retainedIds);
        if (collectionChanged) {
            virtualLayoutRevision = virtualLayout.reset(collection.ids, messageMeasurements.values);
            observedMessageCollection = collection;
        }
        if (!collectionChanged && !pruned && !anchorRemoved) return;
        if (anchorRemoved) {
            stableMeasurementAnchor = retainedAnchor;
            pendingMeasurementAnchor = retainedAnchor;
        }
        const replacementAnchor = nearBottom
            ? undefined
            : (retainedAnchor ??
              (anchorRemoved ? undefined : (captureScrollAnchor() ?? undefined)));
        if (replacementAnchor !== undefined) stableMeasurementAnchor = replacementAnchor;
        scheduleMeasurementFlush(replacementAnchor);
    });

    $effect(() => {
        const messageCount = messageCollection.items.length;
        const liveResponseLength =
            appState.chat.streaming_text.length + appState.chat.reasoning_text.length;
        if (messageCount === anchoredMessageCount && liveResponseLength === 0) {
            return;
        }
        anchoredMessageCount = messageCount;
        if (nearBottom) {
            const epoch = scrollAnchorEpoch;
            void scrollToBottom(epoch);
        }
    });

    $effect(() => {
        const request = messageFocusRequest;
        if (request === null || request.request_id === handledMessageFocusRequestId) return;
        handledMessageFocusRequestId = request.request_id;
        void focusMemorySource(request);
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

    onMount(() => {
        const scroller = messageScroller;
        if (scroller === null || typeof ResizeObserver === 'undefined') {
            return;
        }
        const observer = new ResizeObserver(([entry]) => {
            if (!entry) return;
            const anchorBeforeInvalidation = nearBottom
                ? null
                : (stableMeasurementAnchor ?? captureScrollAnchor());
            viewportHeight = entry.contentRect.height;
            handoffFocusedMessageOutsideWindow(scroller, scroller.scrollTop, viewportHeight);
            refreshNearBottom(scroller, viewportHeight);
            const nextMeasurementEpoch = messageMeasurements.setViewportWidth(
                entry.contentRect.width,
            );
            if (nextMeasurementEpoch === measurementEpoch) return;
            measurementEpoch = nextMeasurementEpoch;
            virtualLayoutRevision = virtualLayout.reset(
                messageCollection.ids,
                messageMeasurements.values,
            );
            scheduleMeasurementFlush(anchorBeforeInvalidation ?? undefined);
        });
        observer.observe(scroller);
        return () => {
            observer.disconnect();
        };
    });

    function deriveRetainedCollectionAnchor(
        removedAnchor: ScrollAnchorSnapshot,
        previousCollection: MessageCollectionSnapshot | null,
        nextCollection: MessageCollectionSnapshot,
    ): ScrollAnchorSnapshot | null {
        if (previousCollection === null) return null;
        const removedIndex = previousCollection.indexesById[removedAnchor.messageId];
        if (
            removedIndex === undefined ||
            virtualLayout.idAt(removedIndex) !== removedAnchor.messageId
        ) {
            return null;
        }
        let retainedIndex = findRetainedMessagePredecessorIndex(
            previousCollection.ids,
            removedIndex,
            nextCollection.retainedIds,
        );
        if (retainedIndex === undefined) {
            for (let index = removedIndex + 1; index < previousCollection.ids.length; index += 1) {
                const messageId = previousCollection.ids[index];
                if (messageId !== undefined && nextCollection.retainedIds.has(messageId)) {
                    retainedIndex = index;
                    break;
                }
            }
        }
        if (retainedIndex === undefined) return null;
        const messageId = previousCollection.ids[retainedIndex];
        if (messageId === undefined) return null;
        const previousVirtualTop =
            VIRTUAL_MESSAGE_BLOCK_PADDING + virtualMessageOffset(virtualLayout, retainedIndex);
        return {
            messageId,
            relativeTop: removedAnchor.relativeTop + previousVirtualTop - removedAnchor.virtualTop,
            scrollTop: removedAnchor.scrollTop,
            virtualTop: previousVirtualTop,
            preservesPreMutationPosition: true,
        };
    }

    function captureScrollAnchor(): ScrollAnchorSnapshot | null {
        if (messageScroller === null) return null;
        const scrollerTop = messageScroller.getBoundingClientRect().top;
        const renderedMessages = Array.from(
            messageScroller.querySelectorAll<HTMLElement>('[data-message-id]'),
        );
        const anchor =
            renderedMessages.find(
                (element) => element.getBoundingClientRect().bottom > scrollerTop,
            ) ?? renderedMessages[0];
        const messageId = anchor?.dataset.messageId;
        if (anchor === undefined || messageId === undefined) return null;
        const messageIndex = messageCollection.indexesById[messageId];
        if (messageIndex === undefined) return null;
        return {
            messageId,
            relativeTop: anchor.getBoundingClientRect().top - scrollerTop,
            scrollTop: messageScroller.scrollTop,
            virtualTop:
                VIRTUAL_MESSAGE_BLOCK_PADDING + virtualMessageOffset(virtualLayout, messageIndex),
        };
    }

    function scheduleMeasurementFlush(anchorBeforeInvalidation?: ScrollAnchorSnapshot): void {
        const nextMeasurementEpoch = messageMeasurements.epoch;
        if (
            !measurementFlushQueued ||
            pendingMeasurementEpoch !== nextMeasurementEpoch ||
            pendingMessageCollectionEpoch !== messageCollectionEpoch
        ) {
            pendingMeasurementEpoch = nextMeasurementEpoch;
            pendingMessageCollectionEpoch = messageCollectionEpoch;
            pendingMeasurementScrollEpoch = scrollAnchorEpoch;
            pendingMeasurementPinBottom = nearBottom;
            pendingMeasurementAnchor = nearBottom
                ? null
                : (anchorBeforeInvalidation ?? stableMeasurementAnchor ?? captureScrollAnchor());
        } else if (nearBottom) {
            pendingMeasurementPinBottom = true;
            pendingMeasurementAnchor = null;
        }
        if (measurementFlushQueued) return;
        measurementFlushQueued = true;
        queueMicrotask(() => void flushMessageMeasurements());
    }

    async function flushMessageMeasurements(): Promise<void> {
        const flushMeasurementEpoch = pendingMeasurementEpoch;
        const flushMessageCollectionEpoch = pendingMessageCollectionEpoch;
        const flushScrollEpoch = pendingMeasurementScrollEpoch;
        const pinBottom = pendingMeasurementPinBottom;
        const anchor = pendingMeasurementAnchor;
        measurementFlushQueued = false;
        pendingMeasurementPinBottom = false;
        pendingMeasurementAnchor = null;
        const scroller = messageScroller;
        if (
            flushMeasurementEpoch !== messageMeasurements.epoch ||
            flushMessageCollectionEpoch !== messageCollectionEpoch ||
            flushScrollEpoch !== scrollAnchorEpoch ||
            scroller === null
        ) {
            return;
        }
        let anchorScrollTop = anchor?.scrollTop ?? 0;
        let anchorRetained = false;
        if (anchor !== null) {
            const anchorIndex = messageCollection.indexesById[anchor.messageId];
            if (anchorIndex !== undefined) {
                anchorRetained = true;
                const nextVirtualTop =
                    VIRTUAL_MESSAGE_BLOCK_PADDING +
                    virtualMessageOffset(virtualLayout, anchorIndex);
                anchorScrollTop = computeAnchoredScrollTop(
                    anchor.scrollTop,
                    anchor.virtualTop,
                    nextVirtualTop,
                );
                applyProgrammaticScrollPosition(scroller, anchorScrollTop);
            } else {
                stableMeasurementAnchor = null;
            }
        }
        await tick();
        if (
            flushMeasurementEpoch !== messageMeasurements.epoch ||
            flushMessageCollectionEpoch !== messageCollectionEpoch ||
            flushScrollEpoch !== scrollAnchorEpoch
        ) {
            return;
        }
        if (pinBottom) {
            applyProgrammaticScrollPosition(scroller, scroller.scrollHeight);
            stableMeasurementAnchor = null;
            return;
        }
        if (anchor === null) {
            stableMeasurementAnchor = nearBottom ? null : captureScrollAnchor();
            return;
        }
        if (!anchorRetained) {
            stableMeasurementAnchor = anchor.preservesPreMutationPosition
                ? null
                : nearBottom
                  ? null
                  : captureScrollAnchor();
            return;
        }
        const target = Array.from(scroller.querySelectorAll<HTMLElement>('[data-message-id]')).find(
            (element) => element.dataset.messageId === anchor.messageId,
        );
        if (target === undefined) {
            stableMeasurementAnchor = anchor.preservesPreMutationPosition
                ? null
                : nearBottom
                  ? null
                  : captureScrollAnchor();
            return;
        }
        const relativeTopAfter =
            target.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
        const anchoredScrollTop = computeAnchoredScrollTop(
            anchorScrollTop,
            anchor.relativeTop,
            relativeTopAfter,
        );
        applyProgrammaticScrollPosition(scroller, anchoredScrollTop);
        stableMeasurementAnchor = nearBottom ? null : captureScrollAnchor();
    }

    function recordMessageMeasurement(epoch: number, messageId: string, height: number): void {
        if (!messageMeasurements.record(epoch, messageId, height)) return;
        if (!virtualLayout.updateMeasuredHeight(messageId, height)) return;
        virtualLayoutRevision = virtualLayout.revision;
        if (messageScroller !== null) {
            handoffFocusedMessageOutsideWindow(
                messageScroller,
                messageScroller.scrollTop,
                messageScroller.clientHeight || viewportHeight,
            );
        }
        scheduleMeasurementFlush();
    }

    function measureMessage(node: HTMLElement, input: MessageMeasurementInput) {
        let observer: ResizeObserver | null = null;
        const connect = (nextInput: MessageMeasurementInput): void => {
            observer?.disconnect();
            observer = null;
            if (typeof ResizeObserver === 'undefined') return;
            const { epoch, messageId } = nextInput;
            observer = new ResizeObserver(([entry]) => {
                if (!entry) return;
                const borderBoxHeight = entry.borderBoxSize[0]?.blockSize;
                const rectHeight = node.getBoundingClientRect().height;
                const height =
                    borderBoxHeight ?? (rectHeight > 0 ? rectHeight : entry.contentRect.height);
                recordMessageMeasurement(epoch, messageId, height);
            });
            observer.observe(node);
        };
        connect(input);
        return {
            update: connect,
            destroy(): void {
                observer?.disconnect();
            },
        };
    }

    function refreshNearBottom(scroller: HTMLDivElement, currentViewportHeight: number): void {
        nearBottom = isVirtualMessageNearBottom(
            scroller.scrollHeight,
            scroller.scrollTop,
            currentViewportHeight,
        );
        if (nearBottom) stableMeasurementAnchor = null;
    }

    function handoffFocusedMessageOutsideWindow(
        scroller: HTMLDivElement,
        nextScrollTop: number,
        nextViewportHeight: number,
    ): void {
        const activeElement = document.activeElement;
        if (!(activeElement instanceof HTMLElement) || !scroller.contains(activeElement)) return;
        const focusedRow = activeElement.closest<HTMLElement>('[data-message-id]');
        const focusedMessageId = focusedRow?.dataset.messageId;
        if (focusedMessageId === undefined) return;
        const focusedIndex = messageCollection.indexesById[focusedMessageId];
        if (focusedIndex === undefined) {
            scroller.focus({ preventScroll: true });
            return;
        }
        const nextWindow = computeVirtualMessageWindow(
            virtualLayout,
            Math.max(0, nextScrollTop - VIRTUAL_MESSAGE_BLOCK_PADDING),
            nextViewportHeight,
        );
        if (virtualWindowContainsIndex(nextWindow, focusedIndex)) return;
        scroller.focus({ preventScroll: true });
    }

    function applyProgrammaticScrollPosition(
        scroller: HTMLDivElement,
        nextScrollTop: number,
    ): void {
        scroller.scrollTop = nextScrollTop;
        const currentViewportHeight = scroller.clientHeight || viewportHeight;
        handoffFocusedMessageOutsideWindow(scroller, scroller.scrollTop, currentViewportHeight);
        scrollTop = scroller.scrollTop;
        refreshNearBottom(scroller, currentViewportHeight);
    }

    async function scrollToBottom(epoch: number): Promise<void> {
        await tick();
        if (epoch !== scrollAnchorEpoch || messageScroller === null) return;
        applyProgrammaticScrollPosition(messageScroller, messageScroller.scrollHeight);
        stableMeasurementAnchor = null;
    }

    async function focusMemorySource(request: MemoryRecordSourceNavigationDto): Promise<void> {
        const index = messageCollection.indexesById[request.start_message_id];
        if (index === undefined) {
            copyNotice = '장기기억 출처 메시지가 현재 로드된 대화 기록에 없습니다.';
            return;
        }
        nearBottom = false;
        ++scrollAnchorEpoch;
        const targetTop =
            VIRTUAL_MESSAGE_BLOCK_PADDING + virtualMessageOffset(virtualLayout, index);
        if (messageScroller !== null) {
            handoffFocusedMessageOutsideWindow(messageScroller, targetTop, viewportHeight);
        }
        scrollTop = targetTop;
        await tick();
        if (messageScroller === null) return;
        applyProgrammaticScrollPosition(messageScroller, targetTop);
        await tick();
        copyNotice =
            request.start_message_id === request.end_message_id
                ? '장기기억 출처 메시지로 이동했습니다.'
                : '장기기억 출처 범위의 첫 메시지로 이동했습니다.';
        await tick();
        const target = Array.from(
            messageScroller.querySelectorAll<HTMLElement>('[data-message-id]'),
        ).find((element) => element.dataset.messageId === request.start_message_id);
        target?.focus();
        target?.scrollIntoView({ block: 'center' });
    }

    function handleScroll(event: Event): void {
        const element = event.currentTarget as HTMLDivElement;
        const currentViewportHeight = element.clientHeight || viewportHeight;
        handoffFocusedMessageOutsideWindow(element, element.scrollTop, currentViewportHeight);
        scrollTop = element.scrollTop;
        viewportHeight = currentViewportHeight;
        refreshNearBottom(element, currentViewportHeight);
        const captureEpoch = ++stableAnchorCaptureEpoch;
        if (nearBottom) {
            stableMeasurementAnchor = null;
            return;
        }
        void captureStableAnchorAfterRender(captureEpoch);
    }

    async function captureStableAnchorAfterRender(captureEpoch: number): Promise<void> {
        await tick();
        if (captureEpoch !== stableAnchorCaptureEpoch || nearBottom) return;
        stableMeasurementAnchor = captureScrollAnchor();
    }

    async function submit(): Promise<void> {
        if (sending || draft.trim().length === 0) return;
        sending = true;
        try {
            const accepted = await controller.sendMessage(draft);
            if (accepted) {
                draft = '';
                if (activeDraftKey !== '') drafts.delete(activeDraftKey);
            }
            attemptApprovalRefreshEpoch += 1;
        } finally {
            sending = false;
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

    async function beginNewGenerationOperation(): Promise<void> {
        controller.beginNewGenerationOperation();
        copyNotice = '새 생성 작업으로 전환했습니다. 같은 입력도 새로운 요청으로 처리됩니다.';
        await tick();
        composerTextarea?.focus();
    }

    const onComposerKeydown: KeyboardEventHandler<HTMLTextAreaElement> = (event) => {
        if (
            shouldSubmitComposer(
                {
                    key: event.key,
                    shiftKey: event.shiftKey,
                    isComposing: event.isComposing,
                },
                compositionActive,
            )
        ) {
            event.preventDefault();
            void submit();
        }
    };

    function setMode(mode: ConversationMode): void {
        void controller.setConversationMode(mode);
    }

    function beginEdit(message: MessageDto): void {
        editingMessageId = message.id;
        editDraft = message.content;
        pendingRemoveId = null;
    }

    async function commitEdit(messageId: string): Promise<void> {
        const accepted = await controller.editUserMessage(messageId, editDraft);
        if (accepted) {
            editingMessageId = null;
            editDraft = '';
        }
    }

    async function copyMessage(message: MessageDto): Promise<void> {
        try {
            await navigator.clipboard.writeText(message.content);
            copyNotice = '메시지를 복사했습니다.';
        } catch {
            copyNotice = '메시지를 복사하지 못했습니다.';
        }
    }
</script>

<section class="pane chat-pane" aria-labelledby="chat-title">
    {#if appState.selected_conversation === null}
        <div class="chat-placeholder state-panel empty">
            <span class="large-mark" aria-hidden="true">✦</span>
            <strong>대화를 선택하세요.</strong>
            <p>메시지와 생성 상태는 로컬 Core에서 복원됩니다.</p>
        </div>
    {:else}
        <header class="chat-header">
            <div>
                <p class="eyebrow">{appState.selected_character?.name ?? 'Character'}</p>
                <h2 id="chat-title">{appState.selected_conversation.title}</h2>
            </div>
            <div class="chat-controls">
                {#if orchestrationState && orchestrationController}
                    <OrchestrationQuickDrawer
                        {appState}
                        {orchestrationState}
                        controller={orchestrationController}
                        onOpen={() => {
                            if (appState.providers.phase === 'idle') {
                                void controller.loadProviders();
                            }
                        }}
                        onOpenStudio={onOpenOrchestrationStudio}
                    />
                {/if}
                <div class="segmented" aria-label="대화 모드">
                    <button
                        type="button"
                        class:active={appState.conversation_state?.selected_mode === 'chat'}
                        aria-pressed={appState.conversation_state?.selected_mode === 'chat'}
                        onclick={() => setMode('chat')}
                    >
                        채팅
                    </button>
                    <button
                        type="button"
                        class:active={appState.conversation_state?.selected_mode === 'story'}
                        aria-pressed={appState.conversation_state?.selected_mode === 'story'}
                        onclick={() => setMode('story')}
                    >
                        스토리
                    </button>
                </div>
                {#if appState.branches.length > 1}
                    <label class="branch-picker">
                        <span>분기</span>
                        <select
                            value={appState.conversation_state?.active_branch_id}
                            onchange={(event) =>
                                void controller.selectBranch(event.currentTarget.value)}
                        >
                            {#each appState.branches as branch, index (branch.id)}
                                <option value={branch.id}>
                                    {branch.title ?? `분기 ${String(index + 1)}`}
                                </option>
                            {/each}
                        </select>
                    </label>
                {/if}
            </div>
        </header>

        {#if client !== undefined && interactionController !== null && interactionState.phase !== 'unavailable'}
            {#if interactionState.phase === 'loading'}
                <div class="interaction-status" role="status">
                    대화 상호작용을 복원하는 중입니다.
                </div>
            {:else if interactionState.error !== null}
                <div class="interaction-status error" role="alert">
                    {interactionState.error}
                </div>
            {:else if interactionState.announcement !== ''}
                <div class="interaction-status" role="status" aria-live="polite">
                    {interactionState.announcement}
                </div>
            {/if}

            {#if interactionState.has_more_expired_proposals}
                <div class="interaction-status error" role="alert">
                    <span>
                        만료된 승인 제안이 더 남아 있습니다. 최신 상태를 모두 정리하기 전에는 다른
                        제안을 결정할 수 없습니다.
                    </span>
                    <button
                        type="button"
                        disabled={interactionState.phase === 'loading'}
                        onclick={() => void interactionController?.reload()}
                    >
                        만료 제안 계속 정리
                    </button>
                </div>
            {/if}

            {#if displayInteractionEffects.length > 0 || interactionState.pending_proposals.length > 0}
                <section class="interaction-surface" aria-labelledby="room-interaction-title">
                    <header>
                        <h3 id="room-interaction-title">대화 상호작용</h3>
                        <span>상태 revision {interactionState.current_state_revision}</span>
                    </header>

                    {#if displayInteractionEffects.length > 0}
                        <ul class="interaction-effects">
                            {#each displayInteractionEffects as interactionEffect (interactionEffect.effect_id)}
                                <li>
                                    {#if interactionEffect.effect.kind === 'show_asset'}
                                        <p class="interaction-label">
                                            {interactionEffect.effect.region} 미디어
                                        </p>
                                        <div class="interaction-media">
                                            <TrustedAsset
                                                {client}
                                                selector={{
                                                    kind: 'asset_id',
                                                    asset_id:
                                                        interactionEffect.effect.asset.asset_id,
                                                }}
                                                expectedKind={interactionEffect.effect.asset.kind}
                                                alt={`${interactionEffect.effect.region} 상호작용 미디어`}
                                                showMetadata
                                            />
                                        </div>
                                    {:else if interactionEffect.effect.kind === 'play_audio'}
                                        <p class="interaction-label">상호작용 오디오</p>
                                        <div class="interaction-audio">
                                            <TrustedAsset
                                                {client}
                                                selector={{
                                                    kind: 'asset_id',
                                                    asset_id:
                                                        interactionEffect.effect.asset.asset_id,
                                                }}
                                                expectedKind="audio"
                                                alt="상호작용 오디오"
                                                showMetadata
                                            />
                                        </div>
                                    {:else if interactionEffect.effect.kind === 'present_choices'}
                                        <fieldset>
                                            <legend>선택지</legend>
                                            <div class="interaction-actions">
                                                {#each interactionEffect.effect.choices as choice (choice.id)}
                                                    <button
                                                        type="button"
                                                        class:primary={interactionEffect.selected_choice_id ===
                                                            choice.id}
                                                        disabled={interactionEffect.choice_status !==
                                                            'pending' ||
                                                            interactionState.busy_effect_id ===
                                                                interactionEffect.effect_id}
                                                        onclick={() =>
                                                            void interactionController?.submitChoice(
                                                                interactionEffect.effect_id,
                                                                choice.id,
                                                            )}
                                                    >
                                                        {choice.label}
                                                    </button>
                                                {/each}
                                            </div>
                                            {#if interactionEffect.choice_status === 'consumed'}
                                                <p class="interaction-label">
                                                    선택 반영됨:
                                                    {interactionEffect.selected_choice_id ??
                                                        '알 수 없음'}
                                                </p>
                                            {:else if interactionEffect.choice_status === 'expired'}
                                                <p class="interaction-label">
                                                    이 선택지는 만료되었습니다.
                                                </p>
                                            {/if}
                                        </fieldset>
                                    {:else if interactionEffect.effect.kind === 'visible_system_event'}
                                        <p>{interactionEffect.effect.text}</p>
                                    {:else if interactionEffect.effect.kind === 'dice_rolled'}
                                        <p>
                                            주사위 {interactionEffect.effect
                                                .count}d{interactionEffect.effect.sides}
                                            {interactionEffect.effect.modifier >= 0 ? '+' : ''}
                                            {interactionEffect.effect.modifier} →
                                            {interactionEffect.effect.rolls.join(', ')} · 합계
                                            {interactionEffect.effect.total}
                                        </p>
                                    {:else if interactionEffect.effect.kind === 'approval_pending'}
                                        <article>
                                            <h4>{interactionEffect.effect.title}</h4>
                                            <p>{interactionEffect.effect.body}</p>
                                            {#if interactionEffect.effect.expires_after_seconds !== null}
                                                <small>
                                                    {interactionEffect.effect
                                                        .expires_after_seconds}초 안에 결정
                                                </small>
                                            {/if}
                                        </article>
                                    {:else if interactionEffect.effect.kind === 'projection_rejected'}
                                        <p class="interaction-label" role="status">
                                            {interactionEffect.effect.reason === 'asset_unavailable'
                                                ? '저장된 미디어 효과를 현재 사용할 수 없어 숨겼습니다.'
                                                : interactionEffect.effect.reason ===
                                                    'unsafe_native_text'
                                                  ? '안전한 표시 범위를 벗어난 저장 효과를 숨겼습니다.'
                                                  : '호환되지 않는 저장 효과를 숨겼습니다.'}
                                        </p>
                                    {/if}
                                </li>
                            {/each}
                        </ul>
                    {/if}

                    {#if interactionState.pending_proposals.length > 0}
                        <section aria-labelledby="interaction-proposals-title">
                            <h4 id="interaction-proposals-title">승인 대기 제안</h4>
                            <ul class="interaction-proposals">
                                {#each interactionState.pending_proposals as item (item.proposal.id)}
                                    <li>
                                        {#if item.proposal.projection_rejection_reason === 'unsafe_native_text'}
                                            <strong>저장 제안 내용을 표시할 수 없음</strong>
                                            <p>
                                                안전한 표시 범위를 벗어난 원문은 숨겼습니다. 이
                                                제안은 거절만 할 수 있습니다.
                                            </p>
                                        {:else}
                                            <strong>{item.proposal.title}</strong>
                                            <p>{item.proposal.body}</p>
                                        {/if}
                                        <div class="interaction-actions">
                                            <button
                                                type="button"
                                                disabled={interactionState.busy_proposal_id !==
                                                    null ||
                                                    interactionState.has_more_expired_proposals}
                                                onclick={() =>
                                                    void interactionController?.decideProposal(
                                                        item.proposal.id,
                                                        'reject',
                                                    )}
                                            >
                                                거절
                                            </button>
                                            <button
                                                class="primary"
                                                type="button"
                                                disabled={interactionState.busy_proposal_id !==
                                                    null ||
                                                    interactionState.has_more_expired_proposals ||
                                                    item.proposal.projection_rejection_reason ===
                                                        'unsafe_native_text'}
                                                onclick={() =>
                                                    void interactionController?.decideProposal(
                                                        item.proposal.id,
                                                        'approve',
                                                    )}
                                            >
                                                승인
                                            </button>
                                        </div>
                                    </li>
                                {/each}
                            </ul>
                        </section>
                    {/if}

                    {#if interactionState.has_older_effects}
                        <p class="interaction-label">
                            이전 상호작용 기록은 전문가 기록 화면에서 확인할 수 있습니다.
                        </p>
                    {/if}
                </section>
            {/if}
        {/if}

        <div
            class="message-scroll"
            aria-label="메시지 기록"
            tabindex="-1"
            style:scroll-behavior="auto"
            bind:this={messageScroller}
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
                        <li
                            class:from-user={message.role === 'user'}
                            class:memory-source-boundary={messageFocusRequest !== null &&
                                (message.id === messageFocusRequest.start_message_id ||
                                    message.id === messageFocusRequest.end_message_id)}
                            class="message-item"
                            data-message-id={message.id}
                            use:measureMessage={{ messageId: message.id, epoch: measurementEpoch }}
                            tabindex="-1"
                            aria-setsize={messageCollection.items.length}
                            aria-posinset={virtualWindow.start + localIndex + 1}
                        >
                            <article
                                class="message-bubble"
                                aria-label={message.role === 'user'
                                    ? '내 메시지'
                                    : message.role === 'assistant'
                                      ? '캐릭터 메시지'
                                      : '시스템 메시지'}
                            >
                                {#if editingMessageId === message.id}
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
                                            bind:value={editDraft}
                                            rows="3"></textarea>
                                        <div>
                                            <button
                                                type="button"
                                                onclick={() => {
                                                    editingMessageId = null;
                                                    editDraft = '';
                                                }}
                                            >
                                                취소
                                            </button>
                                            <button
                                                class="primary"
                                                type="submit"
                                                disabled={editDraft.trim().length === 0}
                                            >
                                                새 분기로 저장
                                            </button>
                                        </div>
                                    </form>
                                {:else}
                                    <p>{message.content}</p>
                                    {#if message.status !== 'complete'}
                                        <span class="message-status">{message.status}</span>
                                    {/if}
                                    <div class="message-actions" aria-label="메시지 작업">
                                        <button
                                            type="button"
                                            onclick={() => void copyMessage(message)}
                                        >
                                            복사
                                        </button>
                                        <button
                                            type="button"
                                            disabled={appState.chat.active_generation_id !== null}
                                            onclick={() => void controller.createBranch(message.id)}
                                        >
                                            여기서 분기
                                        </button>
                                        {#if message.role === 'user'}
                                            <button
                                                type="button"
                                                disabled={appState.chat.active_generation_id !==
                                                    null}
                                                onclick={() => beginEdit(message)}
                                            >
                                                편집
                                            </button>
                                        {:else if message.role === 'assistant'}
                                            <button
                                                type="button"
                                                disabled={appState.chat.active_generation_id !==
                                                    null}
                                                onclick={() =>
                                                    void controller.regenerateAssistantMessage(
                                                        message.id,
                                                    )}
                                            >
                                                재생성
                                            </button>
                                        {/if}
                                        {#if pendingRemoveId === message.id}
                                            <button
                                                class="danger"
                                                type="button"
                                                onclick={() => {
                                                    pendingRemoveId = null;
                                                    void controller.removeMessage(message.id);
                                                }}
                                            >
                                                제거 확인
                                            </button>
                                            <button
                                                type="button"
                                                onclick={() => (pendingRemoveId = null)}
                                            >
                                                취소
                                            </button>
                                        {:else}
                                            <button
                                                type="button"
                                                disabled={appState.chat.active_generation_id !==
                                                    null}
                                                onclick={() => (pendingRemoveId = message.id)}
                                            >
                                                여기부터 제거
                                            </button>
                                        {/if}
                                    </div>
                                {/if}
                            </article>
                        </li>
                    {/each}
                    {#if hasLiveResponse}
                        <li
                            class="message-item streaming-message"
                            style:margin-top={`${String(virtualWindow.bottomSpacer)}px`}
                        >
                            <article class="message-bubble streaming" aria-label="생성 중인 응답">
                                {#if appState.chat.reasoning_text !== ''}
                                    <details class="stream-reasoning" open>
                                        <summary>추론 과정</summary>
                                        <p>{appState.chat.reasoning_text}</p>
                                    </details>
                                {/if}
                                {#if appState.chat.streaming_text !== ''}
                                    <section class="stream-answer" aria-label="생성 중인 답변">
                                        <p>{appState.chat.streaming_text}</p>
                                    </section>
                                {/if}
                                <span class="stream-caret" aria-hidden="true"></span>
                            </article>
                        </li>
                    {/if}
                </ol>
            {/if}
        </div>

        {#if appState.chat.error !== null}
            <div class="state-panel error" role="alert">{appState.chat.error}</div>
        {/if}

        {#if client !== undefined}
            <GenerationAttemptApprovals
                {client}
                conversationId={appState.selected_conversation.id}
                sourceBranchId={appState.conversation_state?.active_branch_id ?? null}
                headingId="chat-generation-attempt-approvals-title"
                refreshEpoch={attemptApprovalRefreshEpoch}
                onRetry={returnToRetainedGenerationInput}
                retryLabel="원래 전송·수정·재생성 확인"
            />
        {/if}

        <div class="generation-operation-actions">
            <button
                class="compact"
                type="button"
                disabled={sending ||
                    appState.chat.phase === 'loading' ||
                    appState.chat.active_generation_id !== null}
                onclick={() => void beginNewGenerationOperation()}
            >
                새 생성 작업
            </button>
        </div>

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

        <div class="chat-live-status" aria-live="polite" aria-atomic="true">
            {appState.chat.reconcile_notice ?? appState.chat.usage_label ?? copyNotice}
        </div>

        <form
            class="composer"
            aria-label="메시지 작성"
            onsubmit={(event) => {
                event.preventDefault();
                void submit();
            }}
        >
            <label class="sr-only" for="chat-draft">메시지</label>
            <textarea
                id="chat-draft"
                bind:this={composerTextarea}
                bind:value={draft}
                rows="1"
                maxlength="131072"
                placeholder="메시지를 입력하세요"
                disabled={appState.chat.phase === 'loading' ||
                    appState.chat.active_generation_id !== null ||
                    appState.conversation_state === null}
                oncompositionstart={() => (compositionActive = true)}
                oncompositionend={() => (compositionActive = false)}
                onkeydown={onComposerKeydown}></textarea>
            {#if appState.chat.active_generation_id !== null}
                <button
                    class="danger compact"
                    type="button"
                    aria-label="응답 생성 취소"
                    onclick={() => void controller.cancelGeneration()}
                >
                    중지
                </button>
            {:else}
                <button
                    class="primary send-button"
                    type="submit"
                    disabled={draft.trim().length === 0 || sending}
                    aria-label="메시지 보내기"
                >
                    ↑
                </button>
            {/if}
        </form>
    {/if}
</section>

<style>
    .interaction-status,
    .interaction-surface {
        margin: 8px clamp(12px, 3vw, 34px) 0;
    }

    .interaction-status {
        color: var(--ink-muted);
        font-size: 0.75rem;
    }

    .interaction-status.error {
        color: var(--danger);
    }

    .interaction-surface {
        max-height: min(42vh, 32rem);
        padding: 12px;
        overflow-y: auto;
        border: 1px solid var(--line);
        border-radius: 14px;
        background: var(--surface-muted);
    }

    .interaction-surface > header,
    .interaction-actions {
        display: flex;
        gap: 8px;
        align-items: center;
        justify-content: space-between;
    }

    .interaction-surface h3,
    .interaction-surface h4,
    .interaction-surface p {
        margin: 0;
    }

    .interaction-surface > header span,
    .interaction-label {
        color: var(--ink-muted);
        font-size: 0.72rem;
    }

    .interaction-effects,
    .interaction-proposals {
        display: grid;
        gap: 8px;
        margin: 10px 0 0;
        padding: 0;
        list-style: none;
    }

    .interaction-effects > li,
    .interaction-proposals > li {
        display: grid;
        gap: 8px;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--surface);
    }

    .interaction-media {
        width: min(100%, 28rem);
        height: min(38vh, 20rem);
        overflow: hidden;
        border-radius: 10px;
    }

    .interaction-audio {
        width: min(100%, 32rem);
        min-height: 3rem;
    }

    .interaction-actions {
        flex-wrap: wrap;
        justify-content: flex-start;
    }

    .memory-query-retry-slot {
        margin: 8px clamp(12px, 3vw, 34px) 0;
    }

    .generation-operation-actions {
        display: flex;
        justify-content: flex-end;
        margin: 8px clamp(12px, 3vw, 34px) 0;
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
