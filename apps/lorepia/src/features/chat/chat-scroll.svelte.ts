import { tick } from 'svelte';

import type { MemoryRecordSourceNavigationDto, MessageDto } from '../../lib/ipc/contracts';
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
    type VirtualWindow,
} from './virtual-window';

export interface MessageMeasurementInput {
    messageId: string;
    epoch: number;
    includesDayDivider: boolean;
}

export interface MessageCollectionSnapshot {
    items: MessageDto[];
    ids: string[];
    retainedIds: ReadonlySet<string>;
    indexesById: Readonly<Record<string, number | undefined>>;
}

export class DisplayMessageProjection {
    #itemsSource: MessageDto[] | null = null;
    #liveAssistantMessageId: string | null = null;
    #items: MessageDto[] = [];

    project(items: MessageDto[], liveAssistantMessageId: string | null): MessageDto[] {
        if (
            this.#itemsSource === items &&
            this.#liveAssistantMessageId === liveAssistantMessageId
        ) {
            return this.#items;
        }
        this.#itemsSource = items;
        this.#liveAssistantMessageId = liveAssistantMessageId;
        this.#items =
            liveAssistantMessageId === null
                ? items
                : items.filter((message) => message.id !== liveAssistantMessageId);
        return this.#items;
    }
}

interface ScrollAnchorSnapshot {
    messageId: string;
    relativeTop: number;
    scrollTop: number;
    virtualTop: number;
    preservesPreMutationPosition?: boolean;
}

interface ChatScrollLifecycleOptions {
    currentCollection(): MessageCollectionSnapshot;
    messageDayKey(value: string): string;
    onMemorySourceMissing(): void;
    onMemorySourceFocused(request: MemoryRecordSourceNavigationDto): void;
}

const KOREAN_WEEKDAYS = ['일요일', '월요일', '화요일', '수요일', '목요일', '금요일', '토요일'];

function parsedMessageDate(value: string): Date | null {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? null : date;
}

export function messageDayKey(value: string): string {
    const date = parsedMessageDate(value);
    if (date === null) return value.slice(0, 10);
    return [
        String(date.getFullYear()),
        String(date.getMonth() + 1).padStart(2, '0'),
        String(date.getDate()).padStart(2, '0'),
    ].join('-');
}

export function formatMessageDay(value: string): string {
    const date = parsedMessageDate(value);
    if (date === null) return '날짜를 확인할 수 없음';
    return `${String(date.getFullYear())}년 ${String(date.getMonth() + 1)}월 ${String(
        date.getDate(),
    )}일 ${KOREAN_WEEKDAYS[date.getDay()] ?? ''}`;
}

export function formatMessageTime(value: string): string {
    const date = parsedMessageDate(value);
    if (date === null) return '--:--';
    return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(
        2,
        '0',
    )}`;
}

function createMessageCollectionSnapshot(items: MessageDto[]): MessageCollectionSnapshot {
    const ids = new Array<string>(items.length);
    const indexesById = Object.create(null) as Record<string, number>;
    for (let index = 0; index < items.length; index += 1) {
        const messageId = items[index]?.id;
        if (messageId === undefined) continue;
        ids[index] = messageId;
        indexesById[messageId] = index;
    }
    return {
        items,
        ids,
        retainedIds: new Set(ids),
        indexesById,
    };
}

export class ChatScrollLifecycle {
    readonly #messageMeasurements = new VirtualMessageMeasurements();
    readonly #virtualLayout = new VirtualMessageLayoutIndex();

    scroller = $state<HTMLDivElement | null>(null);
    measurementEpoch = $state(this.#messageMeasurements.epoch);
    #scrollTop = $state(0);
    #viewportHeight = $state(720);
    #nearBottom = $state(true);
    #virtualLayoutRevision = $state(this.#virtualLayout.revision);

    #scrollAnchorEpoch = 0;
    #anchoredBranchKey = '';
    #anchoredMessageCount = 0;
    #measurementFlushQueued = false;
    #pendingMeasurementEpoch = 0;
    #messageCollectionEpoch = 0;
    #pendingMessageCollectionEpoch = 0;
    #pendingMeasurementScrollEpoch = 0;
    #pendingMeasurementPinBottom = false;
    #pendingMeasurementPreserveScrollPosition = false;
    #pendingMeasurementAnchor: ScrollAnchorSnapshot | null = null;
    #stableMeasurementAnchor: ScrollAnchorSnapshot | null = null;
    #stableAnchorCaptureEpoch = 0;
    #cachedMessageCollection: MessageCollectionSnapshot | null = null;
    #observedMessageCollection: MessageCollectionSnapshot | null = null;
    #stableMessageActionLayoutId: string | null = null;

    constructor(private readonly options: ChatScrollLifecycleOptions) {}

    get nearBottom(): boolean {
        return this.#nearBottom;
    }

    snapshotMessageCollection(items: MessageDto[]): MessageCollectionSnapshot {
        if (this.#cachedMessageCollection?.items === items) return this.#cachedMessageCollection;
        this.#cachedMessageCollection = createMessageCollectionSnapshot(items);
        return this.#cachedMessageCollection;
    }

    virtualWindow(): VirtualWindow {
        void this.#virtualLayoutRevision;
        const messageCollection = this.options.currentCollection();
        const window = computeVirtualMessageWindow(
            this.#virtualLayout,
            Math.max(0, this.#scrollTop - VIRTUAL_MESSAGE_BLOCK_PADDING),
            this.#viewportHeight,
        );
        const firstRenderedMessage = messageCollection.items[window.start];
        if (firstRenderedMessage === undefined) return window;
        const firstRenderedDay = this.options.messageDayKey(firstRenderedMessage.created_at);
        let dayStart = window.start;
        while (dayStart > 0) {
            const previous = messageCollection.items[dayStart - 1];
            if (
                previous === undefined ||
                this.options.messageDayKey(previous.created_at) !== firstRenderedDay
            ) {
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
            topSpacer: virtualMessageOffset(this.#virtualLayout, dayStart),
        };
    }

    stabilizeMessageActionLayout(messageId: string): void {
        this.#stableMessageActionLayoutId = messageId;
    }

    clearStableMessageActionLayout(): void {
        this.#stableMessageActionLayoutId = null;
    }

    syncBranch(nextKey: string, resetTransientActions: () => void): void {
        if (nextKey === this.#anchoredBranchKey) return;
        const messageCollection = this.options.currentCollection();
        const anchorBeforeInvalidation = this.#nearBottom
            ? null
            : (this.#stableMeasurementAnchor ?? this.#captureScrollAnchor());
        this.#anchoredBranchKey = nextKey;
        this.measurementEpoch = this.#messageMeasurements.resetScope(nextKey);
        this.#virtualLayoutRevision = this.#virtualLayout.reset(
            messageCollection.ids,
            this.#messageMeasurements.values,
        );
        this.#anchoredMessageCount = messageCollection.items.length;
        resetTransientActions();
        const epoch = ++this.#scrollAnchorEpoch;
        if (
            anchorBeforeInvalidation !== null &&
            messageCollection.retainedIds.has(anchorBeforeInvalidation.messageId)
        ) {
            this.#scheduleMeasurementFlush(anchorBeforeInvalidation);
        } else {
            this.#stableMeasurementAnchor = null;
            this.#nearBottom = true;
            void this.#scrollToBottom(epoch);
        }
    }

    syncCollection(collection: MessageCollectionSnapshot): void {
        const previousCollection = this.#observedMessageCollection;
        const collectionChanged = previousCollection?.items !== collection.items;
        const anchorBeforeCollectionChange =
            this.#stableMeasurementAnchor ?? this.#pendingMeasurementAnchor;
        const anchorRemoved =
            collectionChanged &&
            anchorBeforeCollectionChange !== null &&
            !collection.retainedIds.has(anchorBeforeCollectionChange.messageId);
        const retainedAnchor = anchorRemoved
            ? this.#deriveRetainedCollectionAnchor(
                  anchorBeforeCollectionChange,
                  previousCollection,
                  collection,
              )
            : anchorBeforeCollectionChange;
        if (collectionChanged) this.#messageCollectionEpoch += 1;
        const pruned = this.#messageMeasurements.prune(collection.retainedIds);
        if (collectionChanged) {
            this.#virtualLayoutRevision = this.#virtualLayout.reset(
                collection.ids,
                this.#messageMeasurements.values,
            );
            this.#observedMessageCollection = collection;
        }
        if (!collectionChanged && !pruned && !anchorRemoved) return;
        if (anchorRemoved) {
            this.#stableMeasurementAnchor = retainedAnchor;
            this.#pendingMeasurementAnchor = retainedAnchor;
        }
        const replacementAnchor = this.#nearBottom
            ? undefined
            : (retainedAnchor ??
              (anchorRemoved ? undefined : (this.#captureScrollAnchor() ?? undefined)));
        if (replacementAnchor !== undefined) this.#stableMeasurementAnchor = replacementAnchor;
        this.#scheduleMeasurementFlush(replacementAnchor);
    }

    syncMessageGrowth(messageCount: number, liveResponseLength: number): void {
        if (messageCount === this.#anchoredMessageCount && liveResponseLength === 0) {
            return;
        }
        this.#anchoredMessageCount = messageCount;
        if (this.#nearBottom) {
            const epoch = this.#scrollAnchorEpoch;
            void this.#scrollToBottom(epoch);
        }
    }

    observeScroller(): (() => void) | undefined {
        const scroller = this.scroller;
        if (scroller === null || typeof ResizeObserver === 'undefined') {
            return;
        }
        const observer = new ResizeObserver(([entry]) => {
            if (!entry) return;
            const anchorBeforeInvalidation = this.#nearBottom
                ? null
                : (this.#stableMeasurementAnchor ?? this.#captureScrollAnchor());
            this.#viewportHeight = entry.contentRect.height;
            this.#handoffFocusedMessageOutsideWindow(
                scroller,
                scroller.scrollTop,
                this.#viewportHeight,
            );
            this.#refreshNearBottom(scroller, this.#viewportHeight);
            const nextMeasurementEpoch = this.#messageMeasurements.setViewportWidth(
                entry.contentRect.width,
            );
            if (nextMeasurementEpoch === this.measurementEpoch) return;
            this.measurementEpoch = nextMeasurementEpoch;
            this.#virtualLayoutRevision = this.#virtualLayout.reset(
                this.options.currentCollection().ids,
                this.#messageMeasurements.values,
            );
            this.#scheduleMeasurementFlush(anchorBeforeInvalidation ?? undefined);
        });
        observer.observe(scroller);
        return () => {
            observer.disconnect();
        };
    }

    #deriveRetainedCollectionAnchor(
        removedAnchor: ScrollAnchorSnapshot,
        previousCollection: MessageCollectionSnapshot | null,
        nextCollection: MessageCollectionSnapshot,
    ): ScrollAnchorSnapshot | null {
        if (previousCollection === null) return null;
        const removedIndex = previousCollection.indexesById[removedAnchor.messageId];
        if (
            removedIndex === undefined ||
            this.#virtualLayout.idAt(removedIndex) !== removedAnchor.messageId
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
            VIRTUAL_MESSAGE_BLOCK_PADDING +
            virtualMessageOffset(this.#virtualLayout, retainedIndex);
        return {
            messageId,
            relativeTop: removedAnchor.relativeTop + previousVirtualTop - removedAnchor.virtualTop,
            scrollTop: removedAnchor.scrollTop,
            virtualTop: previousVirtualTop,
            preservesPreMutationPosition: true,
        };
    }

    #captureScrollAnchor(): ScrollAnchorSnapshot | null {
        if (this.scroller === null) return null;
        const scrollerTop = this.scroller.getBoundingClientRect().top;
        const renderedMessages = Array.from(
            this.scroller.querySelectorAll<HTMLElement>('[data-message-id]'),
        );
        const anchor =
            renderedMessages.find(
                (element) => element.getBoundingClientRect().bottom > scrollerTop,
            ) ?? renderedMessages[0];
        const messageId = anchor?.dataset.messageId;
        if (anchor === undefined || messageId === undefined) return null;
        const messageIndex = this.options.currentCollection().indexesById[messageId];
        if (messageIndex === undefined) return null;
        return {
            messageId,
            relativeTop: anchor.getBoundingClientRect().top - scrollerTop,
            scrollTop: this.scroller.scrollTop,
            virtualTop:
                VIRTUAL_MESSAGE_BLOCK_PADDING +
                virtualMessageOffset(this.#virtualLayout, messageIndex),
        };
    }

    #scheduleMeasurementFlush(
        anchorBeforeInvalidation?: ScrollAnchorSnapshot,
        preserveScrollPosition = false,
    ): void {
        const nextMeasurementEpoch = this.#messageMeasurements.epoch;
        if (
            !this.#measurementFlushQueued ||
            this.#pendingMeasurementEpoch !== nextMeasurementEpoch ||
            this.#pendingMessageCollectionEpoch !== this.#messageCollectionEpoch
        ) {
            this.#pendingMeasurementEpoch = nextMeasurementEpoch;
            this.#pendingMessageCollectionEpoch = this.#messageCollectionEpoch;
            this.#pendingMeasurementScrollEpoch = this.#scrollAnchorEpoch;
            this.#pendingMeasurementPreserveScrollPosition = preserveScrollPosition;
            this.#pendingMeasurementPinBottom = this.#nearBottom && !preserveScrollPosition;
            this.#pendingMeasurementAnchor =
                this.#nearBottom || preserveScrollPosition
                    ? null
                    : (anchorBeforeInvalidation ??
                      this.#stableMeasurementAnchor ??
                      this.#captureScrollAnchor());
        } else if (preserveScrollPosition) {
            this.#pendingMeasurementPreserveScrollPosition = true;
            this.#pendingMeasurementPinBottom = false;
            this.#pendingMeasurementAnchor = null;
        } else if (!this.#pendingMeasurementPreserveScrollPosition && this.#nearBottom) {
            this.#pendingMeasurementPinBottom = true;
            this.#pendingMeasurementAnchor = null;
        }
        if (this.#measurementFlushQueued) return;
        this.#measurementFlushQueued = true;
        queueMicrotask(() => void this.#flushMessageMeasurements());
    }

    async #flushMessageMeasurements(): Promise<void> {
        const flushMeasurementEpoch = this.#pendingMeasurementEpoch;
        const flushMessageCollectionEpoch = this.#pendingMessageCollectionEpoch;
        const flushScrollEpoch = this.#pendingMeasurementScrollEpoch;
        const pinBottom = this.#pendingMeasurementPinBottom;
        const anchor = this.#pendingMeasurementAnchor;
        this.#measurementFlushQueued = false;
        this.#pendingMeasurementPinBottom = false;
        this.#pendingMeasurementPreserveScrollPosition = false;
        this.#pendingMeasurementAnchor = null;
        const scroller = this.scroller;
        if (
            flushMeasurementEpoch !== this.#messageMeasurements.epoch ||
            flushMessageCollectionEpoch !== this.#messageCollectionEpoch ||
            flushScrollEpoch !== this.#scrollAnchorEpoch ||
            scroller === null
        ) {
            return;
        }
        let anchorScrollTop = anchor?.scrollTop ?? 0;
        let anchorRetained = false;
        if (anchor !== null) {
            const anchorIndex = this.options.currentCollection().indexesById[anchor.messageId];
            if (anchorIndex !== undefined) {
                anchorRetained = true;
                const nextVirtualTop =
                    VIRTUAL_MESSAGE_BLOCK_PADDING +
                    virtualMessageOffset(this.#virtualLayout, anchorIndex);
                anchorScrollTop = computeAnchoredScrollTop(
                    anchor.scrollTop,
                    anchor.virtualTop,
                    nextVirtualTop,
                );
                this.applyProgrammaticScrollPosition(scroller, anchorScrollTop);
            } else {
                this.#stableMeasurementAnchor = null;
            }
        }
        await tick();
        if (
            flushMeasurementEpoch !== this.#messageMeasurements.epoch ||
            flushMessageCollectionEpoch !== this.#messageCollectionEpoch ||
            flushScrollEpoch !== this.#scrollAnchorEpoch
        ) {
            return;
        }
        if (pinBottom) {
            this.applyProgrammaticScrollPosition(scroller, scroller.scrollHeight);
            this.#stableMeasurementAnchor = null;
            return;
        }
        if (anchor === null) {
            this.#stableMeasurementAnchor = this.#nearBottom ? null : this.#captureScrollAnchor();
            return;
        }
        if (!anchorRetained) {
            this.#stableMeasurementAnchor = anchor.preservesPreMutationPosition
                ? null
                : this.#nearBottom
                  ? null
                  : this.#captureScrollAnchor();
            return;
        }
        const target = Array.from(scroller.querySelectorAll<HTMLElement>('[data-message-id]')).find(
            (element) => element.dataset.messageId === anchor.messageId,
        );
        if (target === undefined) {
            this.#stableMeasurementAnchor = anchor.preservesPreMutationPosition
                ? null
                : this.#nearBottom
                  ? null
                  : this.#captureScrollAnchor();
            return;
        }
        const relativeTopAfter =
            target.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
        const anchoredScrollTop = computeAnchoredScrollTop(
            anchorScrollTop,
            anchor.relativeTop,
            relativeTopAfter,
        );
        this.applyProgrammaticScrollPosition(scroller, anchoredScrollTop);
        this.#stableMeasurementAnchor = this.#nearBottom ? null : this.#captureScrollAnchor();
    }

    #recordMessageMeasurement(epoch: number, messageId: string, height: number): void {
        if (!this.#messageMeasurements.record(epoch, messageId, height)) return;
        if (!this.#virtualLayout.updateMeasuredHeight(messageId, height)) return;
        this.#virtualLayoutRevision = this.#virtualLayout.revision;
        if (this.scroller !== null) {
            this.#handoffFocusedMessageOutsideWindow(
                this.scroller,
                this.scroller.scrollTop,
                this.scroller.clientHeight || this.#viewportHeight,
            );
        }
        this.#scheduleMeasurementFlush(undefined, messageId === this.#stableMessageActionLayoutId);
    }

    measureMessage(node: HTMLElement, input: MessageMeasurementInput) {
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
                this.#recordMessageMeasurement(epoch, messageId, messageHeight + dividerHeight);
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

    #refreshNearBottom(scroller: HTMLDivElement, currentViewportHeight: number): void {
        this.#nearBottom = isVirtualMessageNearBottom(
            scroller.scrollHeight,
            scroller.scrollTop,
            currentViewportHeight,
        );
        if (this.#nearBottom) this.#stableMeasurementAnchor = null;
    }

    #handoffFocusedMessageOutsideWindow(
        scroller: HTMLDivElement,
        nextScrollTop: number,
        nextViewportHeight: number,
    ): void {
        const activeElement = document.activeElement;
        if (!(activeElement instanceof HTMLElement) || !scroller.contains(activeElement)) return;
        const focusedRow = activeElement.closest<HTMLElement>('[data-message-id]');
        const focusedMessageId = focusedRow?.dataset.messageId;
        if (focusedMessageId === undefined) return;
        const focusedIndex = this.options.currentCollection().indexesById[focusedMessageId];
        if (focusedIndex === undefined) {
            scroller.focus({ preventScroll: true });
            return;
        }
        const nextWindow = computeVirtualMessageWindow(
            this.#virtualLayout,
            Math.max(0, nextScrollTop - VIRTUAL_MESSAGE_BLOCK_PADDING),
            nextViewportHeight,
        );
        if (virtualWindowContainsIndex(nextWindow, focusedIndex)) return;
        scroller.focus({ preventScroll: true });
    }

    applyProgrammaticScrollPosition(scroller: HTMLDivElement, nextScrollTop: number): void {
        scroller.scrollTop = nextScrollTop;
        const currentViewportHeight = scroller.clientHeight || this.#viewportHeight;
        this.#handoffFocusedMessageOutsideWindow(
            scroller,
            scroller.scrollTop,
            currentViewportHeight,
        );
        this.#scrollTop = scroller.scrollTop;
        this.#refreshNearBottom(scroller, currentViewportHeight);
    }

    async #scrollToBottom(epoch: number): Promise<void> {
        await tick();
        if (epoch !== this.#scrollAnchorEpoch || this.scroller === null) return;
        this.applyProgrammaticScrollPosition(this.scroller, this.scroller.scrollHeight);
        this.#stableMeasurementAnchor = null;
    }

    async focusMemorySource(request: MemoryRecordSourceNavigationDto): Promise<void> {
        const index = this.options.currentCollection().indexesById[request.start_message_id];
        if (index === undefined) {
            this.options.onMemorySourceMissing();
            return;
        }
        this.#nearBottom = false;
        ++this.#scrollAnchorEpoch;
        const targetTop =
            VIRTUAL_MESSAGE_BLOCK_PADDING + virtualMessageOffset(this.#virtualLayout, index);
        if (this.scroller !== null) {
            this.#handoffFocusedMessageOutsideWindow(
                this.scroller,
                targetTop,
                this.#viewportHeight,
            );
        }
        this.#scrollTop = targetTop;
        await tick();
        if (this.scroller === null) return;
        this.applyProgrammaticScrollPosition(this.scroller, targetTop);
        await tick();
        this.options.onMemorySourceFocused(request);
        await tick();
        const target = Array.from(
            this.scroller.querySelectorAll<HTMLElement>('[data-message-id]'),
        ).find((element) => element.dataset.messageId === request.start_message_id);
        target?.focus();
        target?.scrollIntoView({ block: 'center' });
    }

    handleScroll(event: Event): void {
        const element = event.currentTarget as HTMLDivElement;
        const currentViewportHeight = element.clientHeight || this.#viewportHeight;
        this.#handoffFocusedMessageOutsideWindow(element, element.scrollTop, currentViewportHeight);
        this.#scrollTop = element.scrollTop;
        this.#viewportHeight = currentViewportHeight;
        this.#refreshNearBottom(element, currentViewportHeight);
        const captureEpoch = ++this.#stableAnchorCaptureEpoch;
        if (this.#nearBottom) {
            this.#stableMeasurementAnchor = null;
            return;
        }
        void this.#captureStableAnchorAfterRender(captureEpoch);
    }

    async #captureStableAnchorAfterRender(captureEpoch: number): Promise<void> {
        await tick();
        if (captureEpoch !== this.#stableAnchorCaptureEpoch || this.#nearBottom) return;
        this.#stableMeasurementAnchor = this.#captureScrollAnchor();
    }
}
