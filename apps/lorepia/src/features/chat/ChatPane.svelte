<script lang="ts">
    import {
        ArrowLeft,
        ArrowUp,
        Check,
        ChevronDown,
        Copy,
        GitBranch,
        Maximize2,
        MessagesSquare,
        Minimize2,
        Pencil,
        Plus,
        RefreshCw,
        Share2,
        Sparkles,
        Trash2,
        X,
    } from '@lucide/svelte';
    import PortableMessage from './PortableMessage.svelte';
    import { onMount, tick } from 'svelte';
    import type { KeyboardEventHandler } from 'svelte/elements';
    import { SvelteMap } from 'svelte/reactivity';

    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import ChoicePopover from '../../components/ChoicePopover.svelte';
    import SegmentedControl from '../../components/SegmentedControl.svelte';
    import type {
        ConversationMode,
        GenerationSelectionInput,
        MemoryRecordSourceNavigationDto,
        MessageDto,
    } from '../../lib/ipc/contracts';
    import { t, tr } from '../../lib/i18n';
    import MemoryQueryRetryPanel from '../orchestration/MemoryQueryRetryPanel.svelte';
    import OrchestrationQuickDrawer from '../orchestration/OrchestrationQuickDrawer.svelte';
    import type {
        OrchestrationController,
        OrchestrationState,
    } from '../orchestration/orchestration-controller';
    import { shouldSubmitComposer } from './composer';
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
    import {
        VIRTUAL_MESSAGE_BLOCK_PADDING,
        VIRTUAL_MESSAGE_DOM_LIMIT,
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
        desktop?: boolean;
        titlebarOverlay?: boolean;
        utilityOpen?: boolean;
        orchestrationState?: OrchestrationState;
        orchestrationController?: OrchestrationController;
        client?: InteractionRoomCapableClient;
        messageFocusRequest?: (MemoryRecordSourceNavigationDto & { request_id: number }) | null;
        onOpenHome?: () => void;
    }

    interface MessageMeasurementInput {
        messageId: string;
        epoch: number;
        includesDayDivider: boolean;
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
        desktop = false,
        titlebarOverlay = false,
        utilityOpen = $bindable(false),
        orchestrationState,
        orchestrationController,
        client,
        messageFocusRequest = null,
        onOpenHome = () => undefined,
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
    let pendingMeasurementPreserveScrollPosition = false;
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
    let activeMessageActionId = $state<string | null>(null);
    let hoveredMessageActionId = $state<string | null>(null);
    let stableMessageActionLayoutId: string | null = null;
    let composerExpanded = $state(false);
    let composerCanFullscreen = $state(false);
    let composerOverflows = $state(false);
    let composerFullscreen = $state(false);
    let utilityView = $state<'tools' | 'settings'>('tools');
    let composerField = $state<HTMLDivElement | null>(null);
    let composerLeadingAction = $state<HTMLButtonElement | null>(null);
    let composerSendButton = $state<HTMLButtonElement | null>(null);
    let fullscreenComposerSurface = $state<HTMLFormElement | null>(null);
    let fullscreenCloseButton = $state<HTMLButtonElement | null>(null);
    let fullscreenSendButton = $state<HTMLButtonElement | null>(null);
    let fullscreenTextRegion = $state<HTMLDivElement | null>(null);
    let fullscreenComposerTextarea = $state<HTMLTextAreaElement | null>(null);
    let utilityOpenPointer: {
        pointerId: number;
        startX: number;
        startY: number;
        lastX: number;
        lastTime: number;
        velocityX: number;
        viewportWidth: number;
    } | null = null;
    let utilityOpenGesture = $state<'idle' | 'tracking' | 'dragging'>('idle');
    let suppressUtilityOpenClickUntil = 0;

    const UTILITY_SWIPE_AXIS_LOCK_PX = 8;
    const UTILITY_SWIPE_COMMIT_MIN_PX = 64;
    const UTILITY_SWIPE_COMMIT_MAX_PX = 120;
    const UTILITY_SWIPE_COMMIT_RATIO = 0.22;
    const UTILITY_SWIPE_FLING_MIN_PX = 32;
    const UTILITY_SWIPE_FLING_VELOCITY = 0.55;

    function resetUtilityOpenGesture(): void {
        utilityOpenPointer = null;
        utilityOpenGesture = 'idle';
    }

    function utilitySwipeCommitDistance(viewportWidth: number): number {
        return Math.min(
            UTILITY_SWIPE_COMMIT_MAX_PX,
            Math.max(UTILITY_SWIPE_COMMIT_MIN_PX, viewportWidth * UTILITY_SWIPE_COMMIT_RATIO),
        );
    }

    function handleUtilityOpenPointerDown(event: PointerEvent): void {
        if (desktop || utilityOpen || composerFullscreen) return;
        if (!event.isPrimary || event.button !== 0) return;
        const target = event.currentTarget as HTMLElement;
        const boundsWidth = target.getBoundingClientRect().width;
        const viewportWidth = Math.max(1, boundsWidth || target.clientWidth || window.innerWidth);
        utilityOpenPointer = {
            pointerId: Number.isFinite(event.pointerId) ? event.pointerId : 1,
            startX: event.clientX,
            startY: event.clientY,
            lastX: event.clientX,
            lastTime: event.timeStamp,
            velocityX: 0,
            viewportWidth,
        };
        utilityOpenGesture = 'tracking';
    }

    function handleUtilityOpenPointerMove(event: PointerEvent): void {
        const pointer = utilityOpenPointer;
        if (event.pointerId !== pointer?.pointerId) return;
        const deltaX = event.clientX - pointer.startX;
        const deltaY = event.clientY - pointer.startY;
        const absoluteX = Math.abs(deltaX);
        const absoluteY = Math.abs(deltaY);
        if (utilityOpenGesture === 'tracking') {
            if (absoluteX < UTILITY_SWIPE_AXIS_LOCK_PX && absoluteY < UTILITY_SWIPE_AXIS_LOCK_PX) {
                return;
            }
            if (deltaX >= 0 || absoluteY >= absoluteX) {
                resetUtilityOpenGesture();
                return;
            }
            if (absoluteX < absoluteY * 1.2) return;
            utilityOpenGesture = 'dragging';
            const target = event.currentTarget as HTMLElement;
            if (typeof target.setPointerCapture === 'function') {
                target.setPointerCapture(pointer.pointerId);
            }
        }
        if (utilityOpenGesture !== 'dragging') return;
        event.preventDefault();
        const elapsed = event.timeStamp - pointer.lastTime;
        if (elapsed > 0) {
            pointer.velocityX = Math.min(0, (event.clientX - pointer.lastX) / elapsed);
        }
        pointer.lastX = event.clientX;
        pointer.lastTime = event.timeStamp;
    }

    function releaseUtilityOpenPointer(event: PointerEvent): void {
        const target = event.currentTarget as HTMLElement;
        if (
            typeof target.hasPointerCapture === 'function' &&
            typeof target.releasePointerCapture === 'function' &&
            target.hasPointerCapture(event.pointerId)
        ) {
            target.releasePointerCapture(event.pointerId);
        }
    }

    function handleUtilityOpenPointerUp(event: PointerEvent): void {
        const pointer = utilityOpenPointer;
        if (event.pointerId !== pointer?.pointerId) return;
        releaseUtilityOpenPointer(event);
        if (utilityOpenGesture !== 'dragging') {
            resetUtilityOpenGesture();
            return;
        }
        event.preventDefault();
        const distance = Math.max(0, pointer.startX - event.clientX);
        const commits =
            distance >= utilitySwipeCommitDistance(pointer.viewportWidth) ||
            (distance >= UTILITY_SWIPE_FLING_MIN_PX &&
                -pointer.velocityX >= UTILITY_SWIPE_FLING_VELOCITY);
        if (commits) {
            utilityView = 'tools';
            utilityOpen = true;
            suppressUtilityOpenClickUntil = Date.now() + 120;
        }
        resetUtilityOpenGesture();
    }

    function handleUtilityOpenPointerCancel(event: PointerEvent): void {
        if (event.pointerId !== utilityOpenPointer?.pointerId) return;
        releaseUtilityOpenPointer(event);
        resetUtilityOpenGesture();
    }

    function handleUtilityOpenClickCapture(event: MouseEvent): void {
        if (Date.now() > suppressUtilityOpenClickUntil) return;
        suppressUtilityOpenClickUntil = 0;
        event.preventDefault();
        event.stopPropagation();
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

    function measureComposer(node: HTMLTextAreaElement, draftValue: string) {
        let observer: ResizeObserver | null = null;
        let observedWidth: number | null = null;
        let scrollAnchorFrame: number | null = null;
        let overlayScrollFrame: number | null = null;
        let updateQueued = false;
        let pendingExpandedMeasurement: boolean | null = null;
        const field = node.closest<HTMLElement>('.composer-field');
        const composer = field?.closest<HTMLElement>('.composer');
        const chatPane = field?.closest<HTMLElement>('.chat-pane');
        const resizeTarget = composer ?? field ?? node;

        if (draftValue.trim().length === 0) {
            composerCanFullscreen = false;
            composerOverflows = false;
        }

        const update = (measureExpanded = composerExpanded): void => {
            const appliesMeasurementLayout =
                measureExpanded && field !== null && !field.classList.contains('expanded');
            if (appliesMeasurementLayout) field.classList.add('measuring');
            const style = getComputedStyle(node);
            const parsedFontSize = Number.parseFloat(style.fontSize);
            const fontSize =
                Number.isFinite(parsedFontSize) && parsedFontSize >= 8 ? parsedFontSize : 16;
            const parsedLineHeight = Number.parseFloat(style.lineHeight);
            const lineHeight =
                Number.isFinite(parsedLineHeight) && parsedLineHeight > fontSize
                    ? parsedLineHeight
                    : fontSize *
                      (Number.isFinite(parsedLineHeight) && parsedLineHeight > 0
                          ? parsedLineHeight
                          : 1.45);
            const padding =
                (Number.parseFloat(style.paddingTop) || 0) +
                (Number.parseFloat(style.paddingBottom) || 0);
            const previousInlineHeight = node.style.height;
            node.style.height = '0px';
            const scrollHeight = Math.max(lineHeight + padding, node.scrollHeight);
            node.style.height = previousInlineHeight;
            const contentHeight = Math.max(lineHeight, scrollHeight - padding);
            const lineCount = Math.max(1, Math.ceil((contentHeight - 1) / lineHeight));
            const textRegion = node.closest<HTMLElement>('.composer-text-region');
            const parsedMaximumHeight = Number.parseFloat(
                textRegion ? getComputedStyle(textRegion).maxHeight : '',
            );
            const maximumHeight =
                Number.isFinite(parsedMaximumHeight) && parsedMaximumHeight >= lineHeight + padding
                    ? parsedMaximumHeight
                    : scrollHeight;
            if (appliesMeasurementLayout) field.classList.remove('measuring');
            const nextTextSize = Math.ceil(Math.min(scrollHeight, maximumHeight));
            field?.style.setProperty('--composer-text-size', `${String(nextTextSize)}px`);
            const overflows = scrollHeight > maximumHeight + 1;
            composerOverflows = overflows;
            if (!overflows) {
                if (scrollAnchorFrame !== null) cancelAnimationFrame(scrollAnchorFrame);
                let remainingFrames = 3;
                const anchorVisibleLines = (): void => {
                    node.scrollTop = 0;
                    remainingFrames -= 1;
                    if (remainingFrames > 0) {
                        scrollAnchorFrame = requestAnimationFrame(anchorVisibleLines);
                        return;
                    }
                    scrollAnchorFrame = null;
                };
                node.scrollTop = 0;
                scrollAnchorFrame = requestAnimationFrame(anchorVisibleLines);
            } else if (scrollAnchorFrame !== null) {
                cancelAnimationFrame(scrollAnchorFrame);
                scrollAnchorFrame = null;
            }
            if (node.value.trim().length === 0) {
                composerCanFullscreen = false;
                return;
            }
            composerCanFullscreen = lineCount >= 2 || overflows;
        };
        const scheduleUpdate = (measureExpanded?: boolean): void => {
            if (measureExpanded !== undefined) {
                pendingExpandedMeasurement = measureExpanded;
            }
            if (updateQueued) return;
            updateQueued = true;
            queueMicrotask(() => {
                updateQueued = false;
                const nextExpandedMeasurement = pendingExpandedMeasurement ?? composerExpanded;
                pendingExpandedMeasurement = null;
                update(nextExpandedMeasurement);
            });
        };
        const handleInput = (): void => update(composerExpanded);
        const handleFocusIn = (event: FocusEvent): void => {
            if (event.relatedTarget instanceof Node && field?.contains(event.relatedTarget)) return;
            if (composerExpanded) return;
            // Resolve the final text height before the single expansion transition starts.
            update(true);
            composerExpanded = true;
        };
        const handleFocusOut = (event: FocusEvent): void => {
            if (event.relatedTarget instanceof Node && field?.contains(event.relatedTarget)) return;
            if (!composerExpanded) return;
            if (desktop) return;
            if (node.value.trim().length > 0) return;
            composerExpanded = false;
            scheduleUpdate(false);
        };
        const syncComposerOverlay = (fieldHeight: number): void => {
            if (
                composer === null ||
                composer === undefined ||
                chatPane === null ||
                chatPane === undefined
            ) {
                return;
            }
            const composerStyle = getComputedStyle(composer);
            const bottomInset = Number.parseFloat(composerStyle.paddingBottom) || 0;
            const verticalPadding =
                (Number.parseFloat(composerStyle.paddingTop) || 0) + bottomInset;
            const overlayHeight = Math.ceil(fieldHeight + verticalPadding);
            chatPane.style.setProperty(
                '--composer-field-height',
                `${String(Math.ceil(fieldHeight))}px`,
            );
            chatPane.style.setProperty(
                '--composer-field-bottom-inset',
                `${String(Math.ceil(bottomInset))}px`,
            );
            const previousOverlayHeight = Number.parseFloat(
                chatPane.style.getPropertyValue('--composer-overlay-height'),
            );
            if (
                Number.isFinite(previousOverlayHeight) &&
                Math.abs(previousOverlayHeight - overlayHeight) < 0.5
            ) {
                return;
            }
            chatPane.style.setProperty('--composer-overlay-height', `${String(overlayHeight)}px`);
            const scroller = messageScroller;
            const composerOwnsFocus = field?.contains(document.activeElement) ?? false;
            const overlayDelta = Number.isFinite(previousOverlayHeight)
                ? overlayHeight - previousOverlayHeight
                : 0;
            if ((!nearBottom && !(composerExpanded && composerOwnsFocus)) || scroller === null) {
                return;
            }
            if (
                activeMessageActionId !== null &&
                composerExpanded &&
                composerOwnsFocus &&
                Number.isFinite(previousOverlayHeight)
            ) {
                const anchoredScrollTop = scroller.scrollTop + overlayDelta;
                applyProgrammaticScrollPosition(scroller, anchoredScrollTop);
                if (overlayScrollFrame !== null) cancelAnimationFrame(overlayScrollFrame);
                overlayScrollFrame = requestAnimationFrame(() => {
                    overlayScrollFrame = null;
                    applyProgrammaticScrollPosition(scroller, anchoredScrollTop);
                });
                return;
            }
            applyProgrammaticScrollPosition(scroller, scroller.scrollHeight);
            if (overlayScrollFrame !== null) cancelAnimationFrame(overlayScrollFrame);
            overlayScrollFrame = requestAnimationFrame(() => {
                overlayScrollFrame = null;
                applyProgrammaticScrollPosition(scroller, scroller.scrollHeight);
            });
        };

        node.addEventListener('input', handleInput);
        node.addEventListener('focus', handleFocusIn);
        node.addEventListener('blur', handleFocusOut);
        field?.addEventListener('focusin', handleFocusIn);
        field?.addEventListener('focusout', handleFocusOut);
        if (typeof ResizeObserver !== 'undefined') {
            observer = new ResizeObserver((entries) => {
                for (const entry of entries) {
                    if (entry.target === field) {
                        const borderBoxHeight = entry.borderBoxSize[0]?.blockSize;
                        syncComposerOverlay(borderBoxHeight ?? entry.contentRect.height);
                        continue;
                    }
                    const nextWidth = entry.contentRect.width;
                    if (observedWidth !== null && Math.abs(nextWidth - observedWidth) < 0.5)
                        continue;
                    observedWidth = nextWidth;
                    scheduleUpdate();
                }
            });
            observer.observe(resizeTarget);
            if (field !== null && field !== resizeTarget) observer.observe(field);
        }
        scheduleUpdate();

        return {
            update(nextDraft: string): void {
                if (nextDraft.trim().length === 0) {
                    composerCanFullscreen = false;
                    composerOverflows = false;
                }
                scheduleUpdate();
            },
            destroy(): void {
                node.removeEventListener('input', handleInput);
                node.removeEventListener('focus', handleFocusIn);
                node.removeEventListener('blur', handleFocusOut);
                field?.removeEventListener('focusin', handleFocusIn);
                field?.removeEventListener('focusout', handleFocusOut);
                observer?.disconnect();
                if (scrollAnchorFrame !== null) cancelAnimationFrame(scrollAnchorFrame);
                if (overlayScrollFrame !== null) cancelAnimationFrame(overlayScrollFrame);
                chatPane?.style.removeProperty('--composer-overlay-height');
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
    const messageCollection = $derived(snapshotMessageCollection(displayMessageItems));
    const virtualLayoutSnapshot = $derived({
        layout: virtualLayout,
        revision: virtualLayoutRevision,
    });
    const virtualWindow = $derived.by(() => {
        const window = computeVirtualMessageWindow(
            virtualLayoutSnapshot.layout,
            Math.max(0, scrollTop - VIRTUAL_MESSAGE_BLOCK_PADDING),
            viewportHeight,
        );
        const firstRenderedMessage = messageCollection.items[window.start];
        if (firstRenderedMessage === undefined) return window;
        const firstRenderedDay = messageDayKey(firstRenderedMessage.created_at);
        let dayStart = window.start;
        while (dayStart > 0) {
            const previous = messageCollection.items[dayStart - 1];
            if (previous === undefined || messageDayKey(previous.created_at) !== firstRenderedDay) {
                break;
            }
            dayStart -= 1;
        }
        if (dayStart === window.start || window.end - dayStart > VIRTUAL_MESSAGE_DOM_LIMIT) {
            return window;
        }
        return {
            ...window,
            start: dayStart,
            topSpacer: virtualMessageOffset(virtualLayoutSnapshot.layout, dayStart),
        };
    });
    const visibleMessages = $derived(
        messageCollection.items.slice(virtualWindow.start, virtualWindow.end),
    );

    $effect(() => {
        const activeMessageId = activeMessageActionId;
        if (
            activeMessageId !== null &&
            messageCollection.indexesById[activeMessageId] === undefined
        ) {
            activeMessageActionId = null;
            stableMessageActionLayoutId = null;
        }
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
        if (nextKey !== activeDraftKey) {
            if (activeDraftKey !== '') drafts.set(activeDraftKey, draft);
            draft = drafts.get(nextKey) ?? '';
            activeDraftKey = nextKey;
            composerExpanded = desktop;
            utilityView = 'tools';
            composerFullscreen = false;
            composerCanFullscreen = false;
            composerOverflows = false;
        }
    });

    $effect(() => {
        if (desktop) {
            composerExpanded = true;
            return;
        }
        const composerOwnsFocus =
            typeof document !== 'undefined' &&
            composerField?.contains(document.activeElement) === true;
        if (!composerOwnsFocus && draft.trim().length === 0) composerExpanded = false;
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

    function scheduleMeasurementFlush(
        anchorBeforeInvalidation?: ScrollAnchorSnapshot,
        preserveScrollPosition = false,
    ): void {
        const nextMeasurementEpoch = messageMeasurements.epoch;
        if (
            !measurementFlushQueued ||
            pendingMeasurementEpoch !== nextMeasurementEpoch ||
            pendingMessageCollectionEpoch !== messageCollectionEpoch
        ) {
            pendingMeasurementEpoch = nextMeasurementEpoch;
            pendingMessageCollectionEpoch = messageCollectionEpoch;
            pendingMeasurementScrollEpoch = scrollAnchorEpoch;
            pendingMeasurementPreserveScrollPosition = preserveScrollPosition;
            pendingMeasurementPinBottom = nearBottom && !preserveScrollPosition;
            pendingMeasurementAnchor =
                nearBottom || preserveScrollPosition
                    ? null
                    : (anchorBeforeInvalidation ??
                      stableMeasurementAnchor ??
                      captureScrollAnchor());
        } else if (preserveScrollPosition) {
            pendingMeasurementPreserveScrollPosition = true;
            pendingMeasurementPinBottom = false;
            pendingMeasurementAnchor = null;
        } else if (!pendingMeasurementPreserveScrollPosition && nearBottom) {
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
        pendingMeasurementPreserveScrollPosition = false;
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
        scheduleMeasurementFlush(undefined, messageId === stableMessageActionLayoutId);
    }

    function measureMessage(node: HTMLElement, input: MessageMeasurementInput) {
        let observer: ResizeObserver | null = null;
        const connect = (nextInput: MessageMeasurementInput): void => {
            observer?.disconnect();
            observer = null;
            if (typeof ResizeObserver === 'undefined') return;
            const { epoch, messageId, includesDayDivider } = nextInput;
            const dayDivider =
                includesDayDivider && node.previousElementSibling instanceof HTMLElement
                    ? node.previousElementSibling.matches('.message-date-divider')
                        ? node.previousElementSibling
                        : null
                    : null;
            observer = new ResizeObserver((entries) => {
                const nodeEntry = entries.find((entry) => entry.target === node);
                const borderBoxHeight = nodeEntry?.borderBoxSize[0]?.blockSize;
                const rectHeight = node.getBoundingClientRect().height;
                const messageHeight =
                    rectHeight > 0
                        ? rectHeight
                        : (borderBoxHeight ?? nodeEntry?.contentRect.height ?? 0);
                const dividerHeight = dayDivider?.getBoundingClientRect().height ?? 0;
                recordMessageMeasurement(epoch, messageId, messageHeight + dividerHeight);
            });
            observer.observe(node);
            if (dayDivider !== null) observer.observe(dayDivider);
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

    function handleMessageScrollPointerDown(event: PointerEvent): void {
        const target = event.target;
        if (target instanceof Element && target.closest('[data-message-id]') !== null) return;
        activeMessageActionId = null;
    }

    function activateMessageActions(messageId: string): void {
        stableMessageActionLayoutId = messageId;
        activeMessageActionId = messageId;
    }

    function hoverMessageActions(messageId: string): void {
        if (!desktop) return;
        hoveredMessageActionId = messageId;
    }

    function unhoverMessageActions(messageId: string): void {
        if (hoveredMessageActionId === messageId) hoveredMessageActionId = null;
    }

    async function captureStableAnchorAfterRender(captureEpoch: number): Promise<void> {
        await tick();
        if (captureEpoch !== stableAnchorCaptureEpoch || nearBottom) return;
        stableMeasurementAnchor = captureScrollAnchor();
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
        if (sending || draft.trim().length === 0) return;
        sending = true;
        try {
            let content = draft;
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
                draft = '';
                composerFullscreen = false;
                if (activeDraftKey !== '') drafts.delete(activeDraftKey);
            }
            attemptApprovalRefreshEpoch += 1;
        } catch (error) {
            copyNotice = portableRuntimeLifecycle.fail(error, t('chat.runtime.execution_failed'));
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

    const onFullscreenComposerKeydown: KeyboardEventHandler<HTMLTextAreaElement> = (event) => {
        if (event.key === 'Escape') {
            event.preventDefault();
            void setComposerFullscreen(false);
            return;
        }
        onComposerKeydown(event);
    };

    function setComposerControlOrigin(
        source: HTMLButtonElement | null,
        target: HTMLButtonElement | null,
    ): void {
        if (source === null || target === null) return;

        const sourceRect = source.getBoundingClientRect();
        const previousTransition = target.style.transition;
        const previousTransform = target.style.transform;

        // Measure the target at its layout position rather than at a previous
        // morph offset, then install the new start point without painting it.
        target.style.transition = 'none';
        target.style.transform = 'none';
        const targetRect = target.getBoundingClientRect();
        const originX =
            sourceRect.left + sourceRect.width / 2 - (targetRect.left + targetRect.width / 2);
        const originY =
            sourceRect.top + sourceRect.height / 2 - (targetRect.top + targetRect.height / 2);

        target.style.setProperty('--composer-control-origin-x', `${String(originX)}px`);
        target.style.setProperty('--composer-control-origin-y', `${String(originY)}px`);
        if (previousTransform === '') target.style.removeProperty('transform');
        else target.style.transform = previousTransform;
        target.getBoundingClientRect();
        if (previousTransition === '') target.style.removeProperty('transition');
        else target.style.transition = previousTransition;
    }

    function syncFullscreenControlOrigins(): void {
        setComposerControlOrigin(composerLeadingAction, fullscreenCloseButton);
        setComposerControlOrigin(composerSendButton, fullscreenSendButton);
    }

    function syncFullscreenTextOrigin(): void {
        const source = composerTextarea;
        const target = fullscreenComposerTextarea;
        const targetRegion = fullscreenTextRegion;
        if (source === null || target === null || targetRegion === null) return;

        const sourceRect = source.getBoundingClientRect();
        const sourceStyle = getComputedStyle(source);
        const previousTransition = targetRegion.style.transition;
        const previousTransform = targetRegion.style.transform;

        targetRegion.style.transition = 'none';
        targetRegion.style.transform = 'none';
        const targetRect = target.getBoundingClientRect();
        const targetStyle = getComputedStyle(target);
        const sourceTextLeft = sourceRect.left + (Number.parseFloat(sourceStyle.paddingLeft) || 0);
        const sourceTextTop = sourceRect.top + (Number.parseFloat(sourceStyle.paddingTop) || 0);
        const targetTextLeft = targetRect.left + (Number.parseFloat(targetStyle.paddingLeft) || 0);
        const targetTextTop = targetRect.top + (Number.parseFloat(targetStyle.paddingTop) || 0);

        targetRegion.style.setProperty(
            '--composer-text-origin-x',
            `${String(sourceTextLeft - targetTextLeft)}px`,
        );
        targetRegion.style.setProperty(
            '--composer-text-origin-y',
            `${String(sourceTextTop - targetTextTop)}px`,
        );
        targetRegion.style.setProperty('--composer-text-origin-font-size', sourceStyle.fontSize);
        targetRegion.style.setProperty(
            '--composer-text-origin-line-height',
            sourceStyle.lineHeight,
        );
        if (previousTransform === '') targetRegion.style.removeProperty('transform');
        else targetRegion.style.transform = previousTransform;
        targetRegion.getBoundingClientRect();
        if (previousTransition === '') targetRegion.style.removeProperty('transition');
        else targetRegion.style.transition = previousTransition;
    }

    function syncFullscreenSurfaceOrigin(): void {
        const source = composerField;
        const target = fullscreenComposerSurface;
        if (source === null || target === null) return;

        const sourceRect = source.getBoundingClientRect();
        const targetRect = target.getBoundingClientRect();
        const originTop = Math.max(0, sourceRect.top - targetRect.top);
        const originRight = Math.max(0, targetRect.right - sourceRect.right);
        const originBottom = Math.max(0, targetRect.bottom - sourceRect.bottom);
        const originLeft = Math.max(0, sourceRect.left - targetRect.left);
        const originRadius = getComputedStyle(source).borderTopLeftRadius || '0px';

        target.style.setProperty('--composer-origin-top', `${String(originTop)}px`);
        target.style.setProperty('--composer-origin-right', `${String(originRight)}px`);
        target.style.setProperty('--composer-origin-bottom', `${String(originBottom)}px`);
        target.style.setProperty('--composer-origin-left', `${String(originLeft)}px`);
        target.style.setProperty('--composer-origin-radius', originRadius);
    }

    async function setComposerFullscreen(nextOpen: boolean): Promise<void> {
        if (nextOpen && !composerCanFullscreen) return;
        if (composerFullscreen === nextOpen) return;
        syncFullscreenSurfaceOrigin();
        syncFullscreenControlOrigins();
        syncFullscreenTextOrigin();
        composerFullscreen = nextOpen;
        await tick();
        const target = nextOpen ? fullscreenComposerTextarea : composerTextarea;
        target?.focus({ preventScroll: true });
        const cursor = target?.value.length ?? 0;
        target?.setSelectionRange(cursor, cursor);
    }

    function setMode(mode: ConversationMode): void {
        void controller.setConversationMode(mode);
    }

    function beginEdit(message: MessageDto): void {
        editingMessageId = message.id;
        editDraft = message.content;
        pendingRemoveId = null;
    }

    async function removeMessageAndResetRuntime(messageId: string): Promise<void> {
        const result = await controller.removeMessage(messageId);
        if (result.mutationCommitted && branchKey === result.scopeKey) {
            portableRuntimeLifecycle.resetScope();
        }
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
            disabled={sending ||
                appState.chat.phase === 'loading' ||
                appState.chat.active_generation_id !== null}
            onclick={async () => {
                controller.beginNewGenerationOperation();
                copyNotice =
                    '새 생성 작업으로 전환했습니다. 같은 입력도 새로운 요청으로 처리됩니다.';
                await closeSettings();
                await tick();
                composerTextarea?.focus();
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
    data-utility-open-gesture={utilityOpenGesture}
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
                    <OrchestrationQuickDrawer
                        {appState}
                        {orchestrationState}
                        controller={orchestrationController}
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
            bind:this={messageScroller}
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
                            class:actions-open={activeMessageActionId === message.id}
                            class:actions-hovered={hoveredMessageActionId === message.id}
                            class:memory-source-boundary={messageFocusRequest !== null &&
                                (message.id === messageFocusRequest.start_message_id ||
                                    message.id === messageFocusRequest.end_message_id)}
                            class="message-item"
                            data-message-id={message.id}
                            use:measureMessage={{
                                messageId: message.id,
                                epoch: measurementEpoch,
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
                            {#if editingMessageId === message.id}
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
                                <div class="message-actions" aria-label="메시지 작업">
                                    <button
                                        type="button"
                                        aria-label="복사"
                                        title="복사"
                                        onclick={() => void copyMessage(message)}
                                    >
                                        <Copy aria-hidden="true" />
                                    </button>
                                    <button
                                        type="button"
                                        aria-label="여기서 분기"
                                        title="여기서 분기"
                                        disabled={appState.chat.active_generation_id !== null}
                                        onclick={() => void controller.createBranch(message.id)}
                                    >
                                        <GitBranch aria-hidden="true" />
                                    </button>
                                    {#if message.role === 'user'}
                                        <button
                                            type="button"
                                            aria-label="편집"
                                            title="편집"
                                            disabled={appState.chat.active_generation_id !== null}
                                            onclick={() => beginEdit(message)}
                                        >
                                            <Pencil aria-hidden="true" />
                                        </button>
                                    {:else if message.role === 'assistant'}
                                        <button
                                            type="button"
                                            aria-label="재생성"
                                            title="재생성"
                                            disabled={appState.chat.active_generation_id !== null}
                                            onclick={() =>
                                                void controller.regenerateAssistantMessage(
                                                    message.id,
                                                )}
                                        >
                                            <RefreshCw aria-hidden="true" />
                                        </button>
                                    {/if}
                                    {#if pendingRemoveId === message.id}
                                        <button
                                            class="danger"
                                            type="button"
                                            aria-label={$tr('chat.message.remove_confirm')}
                                            title={$tr('chat.message.remove_confirm')}
                                            onclick={() => {
                                                pendingRemoveId = null;
                                                void removeMessageAndResetRuntime(message.id);
                                            }}
                                        >
                                            <Check aria-hidden="true" />
                                        </button>
                                        <button
                                            type="button"
                                            aria-label="취소"
                                            title="취소"
                                            onclick={() => (pendingRemoveId = null)}
                                        >
                                            <X aria-hidden="true" />
                                        </button>
                                    {:else}
                                        <button
                                            type="button"
                                            aria-label={$tr('chat.message.remove_from_here')}
                                            title={$tr('chat.message.remove_from_here')}
                                            disabled={appState.chat.active_generation_id !== null}
                                            onclick={() => (pendingRemoveId = message.id)}
                                        >
                                            <Trash2 aria-hidden="true" />
                                        </button>
                                    {/if}
                                </div>
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

        <form
            class="composer"
            aria-label="메시지 작성"
            aria-hidden={composerFullscreen}
            inert={composerFullscreen}
            onsubmit={(event) => {
                event.preventDefault();
                void submit();
            }}
        >
            <div
                class="composer-field"
                bind:this={composerField}
                class:has-draft={draft.trim().length > 0}
                class:expanded={composerExpanded}
                class:can-fullscreen={composerCanFullscreen}
                class:overflows={composerOverflows}
            >
                <div class="composer-text-region">
                    <textarea
                        id="chat-draft"
                        aria-label="메시지"
                        bind:this={composerTextarea}
                        bind:value={draft}
                        use:measureComposer={draft}
                        rows="1"
                        maxlength="131072"
                        placeholder={desktop ? '무엇이든 요청하세요' : undefined}
                        disabled={appState.chat.phase === 'loading' ||
                            appState.chat.active_generation_id !== null ||
                            appState.conversation_state === null}
                        oncompositionstart={() => (compositionActive = true)}
                        oncompositionend={() => (compositionActive = false)}
                        onkeydown={onComposerKeydown}></textarea>
                </div>
                <div class="composer-action-row">
                    <button
                        class="composer-leading-action"
                        bind:this={composerLeadingAction}
                        type="button"
                        aria-label="추가"
                        onclick={() => composerTextarea?.focus()}
                    >
                        <Plus aria-hidden="true" />
                    </button>
                    <span class="composer-action-spacer" aria-hidden="true"></span>
                    {#if desktop && composerConfigurationLabel !== ''}
                        <button
                            class="composer-desktop-model"
                            type="button"
                            title={composerConfigurationLabel}
                            aria-label={`생성 설정: ${composerConfigurationLabel}`}
                            onclick={openConversationSettings}
                        >
                            <span>{composerConfigurationLabel}</span>
                            <ChevronDown aria-hidden="true" />
                        </button>
                    {/if}
                    <button
                        class="composer-expand-action"
                        class:available={composerCanFullscreen}
                        type="button"
                        aria-label="전체화면으로 작성"
                        aria-hidden={!composerCanFullscreen}
                        tabindex={composerCanFullscreen ? 0 : -1}
                        disabled={!composerCanFullscreen}
                        onclick={() => void setComposerFullscreen(true)}
                    >
                        <Maximize2 aria-hidden="true" />
                    </button>
                    {#if appState.chat.active_generation_id !== null}
                        <button
                            class="danger compact composer-trailing-action"
                            type="button"
                            aria-label="응답 생성 취소"
                            onclick={() => void controller.cancelGeneration()}
                        >
                            중지
                        </button>
                    {:else if draft.trim().length > 0 || desktop}
                        <button
                            class="primary send-button composer-trailing-action"
                            bind:this={composerSendButton}
                            type="submit"
                            disabled={sending || draft.trim().length === 0}
                            aria-label="메시지 보내기"
                        >
                            <ArrowUp class="chat-send-icon" aria-hidden="true" />
                        </button>
                    {/if}
                </div>
            </div>
        </form>

        <form
            class="composer-fullscreen"
            bind:this={fullscreenComposerSurface}
            class:open={composerFullscreen}
            aria-label="전체화면 메시지 작성"
            aria-hidden={!composerFullscreen}
            inert={!composerFullscreen}
            onsubmit={(event) => {
                event.preventDefault();
                void submit();
            }}
        >
            <header class="composer-fullscreen-header">
                <button
                    class="composer-fullscreen-close"
                    bind:this={fullscreenCloseButton}
                    type="button"
                    aria-label="전체화면 입력 닫기"
                    onclick={() => void setComposerFullscreen(false)}
                >
                    <Minimize2 aria-hidden="true" />
                </button>
                <span aria-hidden="true"></span>
                {#if draft.trim().length > 0}
                    <button
                        class="primary send-button"
                        bind:this={fullscreenSendButton}
                        type="submit"
                        disabled={sending}
                        aria-label="메시지 보내기"
                    >
                        <ArrowUp class="chat-send-icon" aria-hidden="true" />
                    </button>
                {/if}
            </header>
            <div class="composer-fullscreen-text-region" bind:this={fullscreenTextRegion}>
                <label class="sr-only" for="chat-draft-fullscreen">전체화면 메시지</label>
                <textarea
                    id="chat-draft-fullscreen"
                    bind:this={fullscreenComposerTextarea}
                    bind:value={draft}
                    maxlength="131072"
                    disabled={appState.chat.phase === 'loading' ||
                        appState.chat.active_generation_id !== null ||
                        appState.conversation_state === null}
                    oncompositionstart={() => (compositionActive = true)}
                    oncompositionend={() => (compositionActive = false)}
                    onkeydown={onFullscreenComposerKeydown}></textarea>
            </div>
        </form>
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
