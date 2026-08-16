export const VIRTUAL_MESSAGE_ESTIMATED_HEIGHT = 96;
export const VIRTUAL_MESSAGE_GAP = 12;
export const VIRTUAL_MESSAGE_BLOCK_PADDING = 22;
export const VIRTUAL_MESSAGE_OVERSCAN = 8;
export const VIRTUAL_MESSAGE_DOM_LIMIT = 80;

const MEASUREMENT_EPSILON = 0.5;

export interface VirtualMessageLayout {
    heights: number[];
    offsets: number[];
    totalHeight: number;
}

export interface VirtualWindow {
    start: number;
    end: number;
    topSpacer: number;
    bottomSpacer: number;
}

interface VirtualMessageLayoutOptions {
    estimatedHeight?: number;
    gap?: number;
}

function finiteNonNegative(value: number, fallback: number): number {
    return Number.isFinite(value) ? Math.max(0, value) : fallback;
}

function finitePositive(value: number, fallback: number): number {
    return Number.isFinite(value) && value > 0 ? value : fallback;
}

export class VirtualMessageMeasurements {
    private readonly measuredHeights = new Map<string, number>();
    private scopeKey = '';
    private viewportWidth: number | null = null;
    private currentEpoch = 0;

    get epoch(): number {
        return this.currentEpoch;
    }

    get values(): ReadonlyMap<string, number> {
        return this.measuredHeights;
    }

    resetScope(scopeKey: string): number {
        if (scopeKey === this.scopeKey) return this.currentEpoch;
        this.scopeKey = scopeKey;
        this.measuredHeights.clear();
        return ++this.currentEpoch;
    }

    setViewportWidth(width: number): number {
        if (!Number.isFinite(width) || width <= 0) return this.currentEpoch;
        if (this.viewportWidth === null) {
            this.viewportWidth = width;
            return this.currentEpoch;
        }
        if (Math.abs(this.viewportWidth - width) < MEASUREMENT_EPSILON) {
            return this.currentEpoch;
        }
        this.viewportWidth = width;
        this.measuredHeights.clear();
        return ++this.currentEpoch;
    }

    record(epoch: number, messageId: string, height: number): boolean {
        if (
            epoch !== this.currentEpoch ||
            messageId.length === 0 ||
            !Number.isFinite(height) ||
            height <= 0
        ) {
            return false;
        }
        const previous = this.measuredHeights.get(messageId);
        if (previous !== undefined && Math.abs(previous - height) < MEASUREMENT_EPSILON) {
            return false;
        }
        this.measuredHeights.set(messageId, height);
        return true;
    }

    prune(retainedMessageIds: ReadonlySet<string>): boolean {
        let changed = false;
        for (const messageId of this.measuredHeights.keys()) {
            if (retainedMessageIds.has(messageId)) continue;
            this.measuredHeights.delete(messageId);
            changed = true;
        }
        return changed;
    }
}

export class VirtualMessageLayoutIndex {
    private messageIds: readonly string[] = [];
    private indexesById: Readonly<Record<string, number | undefined>> = Object.create(
        null,
    ) as Record<string, number>;
    private heights: number[] = [];
    private prefixTree: number[] = [0];
    private readonly estimatedHeight: number;
    private readonly gap: number;
    private currentRevision = 0;

    constructor(
        messageIds: readonly string[] = [],
        measuredHeights: ReadonlyMap<string, number> = new Map(),
        options: VirtualMessageLayoutOptions = {},
    ) {
        this.estimatedHeight = finitePositive(
            options.estimatedHeight ?? VIRTUAL_MESSAGE_ESTIMATED_HEIGHT,
            VIRTUAL_MESSAGE_ESTIMATED_HEIGHT,
        );
        this.gap = finiteNonNegative(options.gap ?? VIRTUAL_MESSAGE_GAP, VIRTUAL_MESSAGE_GAP);
        this.reset(messageIds, measuredHeights);
    }

    get revision(): number {
        return this.currentRevision;
    }

    get size(): number {
        return this.heights.length;
    }

    get totalHeight(): number {
        return this.prefixSum(this.size);
    }

    reset(messageIds: readonly string[], measuredHeights: ReadonlyMap<string, number>): number {
        this.messageIds = messageIds;
        const indexesById = Object.create(null) as Record<string, number>;
        const heights = new Array<number>(messageIds.length);
        const prefixTree = new Array<number>(messageIds.length + 1).fill(0);
        for (let index = 0; index < messageIds.length; index += 1) {
            const messageId = messageIds[index];
            if (messageId === undefined) continue;
            indexesById[messageId] = index;
            heights[index] = finitePositive(
                measuredHeights.get(messageId) ?? this.estimatedHeight,
                this.estimatedHeight,
            );
        }
        for (let treeIndex = 1; treeIndex <= messageIds.length; treeIndex += 1) {
            const itemIndex = treeIndex - 1;
            prefixTree[treeIndex] =
                (prefixTree[treeIndex] ?? 0) +
                (heights[itemIndex] ?? this.estimatedHeight) +
                (itemIndex + 1 < messageIds.length ? this.gap : 0);
            const parent = treeIndex + (treeIndex & -treeIndex);
            if (parent <= messageIds.length) {
                prefixTree[parent] = (prefixTree[parent] ?? 0) + (prefixTree[treeIndex] ?? 0);
            }
        }
        this.indexesById = indexesById;
        this.heights = heights;
        this.prefixTree = prefixTree;
        return ++this.currentRevision;
    }

    indexOf(messageId: string): number | undefined {
        return this.indexesById[messageId];
    }

    idAt(index: number): string | undefined {
        return this.messageIds[index];
    }

    heightAt(index: number): number {
        if (index < 0 || index >= this.size) return 0;
        return this.heights[index] ?? this.estimatedHeight;
    }

    offsetAt(index: number): number {
        return this.prefixSum(Math.max(0, Math.min(this.size, Math.trunc(index))));
    }

    updateMeasuredHeight(messageId: string, height: number): boolean {
        const index = this.indexOf(messageId);
        if (index === undefined || !Number.isFinite(height) || height <= 0) return false;
        const previous = this.heights[index] ?? this.estimatedHeight;
        if (Math.abs(previous - height) < MEASUREMENT_EPSILON) return false;
        this.heights[index] = height;
        this.addToPrefixTree(index, height - previous);
        this.currentRevision += 1;
        return true;
    }

    private addToPrefixTree(index: number, delta: number): void {
        for (
            let treeIndex = index + 1;
            treeIndex <= this.size;
            treeIndex += treeIndex & -treeIndex
        ) {
            this.prefixTree[treeIndex] = (this.prefixTree[treeIndex] ?? 0) + delta;
        }
    }

    private prefixSum(endExclusive: number): number {
        let total = 0;
        for (let treeIndex = endExclusive; treeIndex > 0; treeIndex -= treeIndex & -treeIndex) {
            total += this.prefixTree[treeIndex] ?? 0;
        }
        return total;
    }
}

export function buildVirtualMessageLayout(
    messageIds: readonly string[],
    measuredHeights: ReadonlyMap<string, number>,
    options: VirtualMessageLayoutOptions = {},
): VirtualMessageLayout {
    const estimatedHeight = finitePositive(
        options.estimatedHeight ?? VIRTUAL_MESSAGE_ESTIMATED_HEIGHT,
        VIRTUAL_MESSAGE_ESTIMATED_HEIGHT,
    );
    const gap = finiteNonNegative(options.gap ?? VIRTUAL_MESSAGE_GAP, VIRTUAL_MESSAGE_GAP);
    const heights = new Array<number>(messageIds.length);
    const offsets = new Array<number>(messageIds.length);
    let nextOffset = 0;

    for (let index = 0; index < messageIds.length; index += 1) {
        const messageId = messageIds[index];
        const measured = messageId === undefined ? undefined : measuredHeights.get(messageId);
        const height = finitePositive(measured ?? estimatedHeight, estimatedHeight);
        offsets[index] = nextOffset;
        heights[index] = height;
        nextOffset += height;
        if (index + 1 < messageIds.length) nextOffset += gap;
    }

    return { heights, offsets, totalHeight: nextOffset };
}

type VirtualMessageLayoutSource = VirtualMessageLayout | VirtualMessageLayoutIndex;

function layoutSize(layout: VirtualMessageLayoutSource): number {
    return layout instanceof VirtualMessageLayoutIndex ? layout.size : layout.heights.length;
}

function layoutHeightAt(layout: VirtualMessageLayoutSource, index: number): number {
    return layout instanceof VirtualMessageLayoutIndex
        ? layout.heightAt(index)
        : (layout.heights[index] ?? 0);
}

function layoutOffsetAt(layout: VirtualMessageLayoutSource, index: number): number {
    return layout instanceof VirtualMessageLayoutIndex
        ? layout.offsetAt(index)
        : (layout.offsets[index] ?? 0);
}

function layoutTotalHeight(layout: VirtualMessageLayoutSource): number {
    return layout instanceof VirtualMessageLayoutIndex ? layout.totalHeight : layout.totalHeight;
}

function firstItemEndingAfter(layout: VirtualMessageLayoutSource, offset: number): number {
    let lower = 0;
    let upper = layoutSize(layout);
    while (lower < upper) {
        const middle = lower + Math.floor((upper - lower) / 2);
        const itemEnd = layoutOffsetAt(layout, middle) + layoutHeightAt(layout, middle);
        if (itemEnd > offset) upper = middle;
        else lower = middle + 1;
    }
    return lower;
}

function firstItemStartingAtOrAfter(layout: VirtualMessageLayoutSource, offset: number): number {
    let lower = 0;
    let upper = layoutSize(layout);
    while (lower < upper) {
        const middle = lower + Math.floor((upper - lower) / 2);
        if (layoutOffsetAt(layout, middle) >= offset) upper = middle;
        else lower = middle + 1;
    }
    return lower;
}

export function computeVirtualMessageWindow(
    layout: VirtualMessageLayoutSource,
    scrollTop: number,
    viewportHeight: number,
): VirtualWindow {
    const total = layoutSize(layout);
    if (total === 0) return { start: 0, end: 0, topSpacer: 0, bottomSpacer: 0 };

    const safeScrollTop = finiteNonNegative(scrollTop, 0);
    const safeViewportHeight = finitePositive(viewportHeight, VIRTUAL_MESSAGE_ESTIMATED_HEIGHT);
    const firstVisible = Math.min(total - 1, firstItemEndingAfter(layout, safeScrollTop));
    const visibleEnd = Math.max(
        firstVisible + 1,
        firstItemStartingAtOrAfter(layout, safeScrollTop + safeViewportHeight),
    );
    const start = Math.max(0, firstVisible - VIRTUAL_MESSAGE_OVERSCAN);
    const requestedEnd = Math.min(total, visibleEnd + VIRTUAL_MESSAGE_OVERSCAN);
    const end = Math.min(requestedEnd, start + VIRTUAL_MESSAGE_DOM_LIMIT);
    const renderedEnd = layoutOffsetAt(layout, end - 1) + layoutHeightAt(layout, end - 1);

    return {
        start,
        end,
        topSpacer: layoutOffsetAt(layout, start),
        bottomSpacer: Math.max(0, layoutTotalHeight(layout) - renderedEnd),
    };
}

export function virtualMessageOffset(layout: VirtualMessageLayoutSource, index: number): number {
    const size = layoutSize(layout);
    if (size === 0) return 0;
    const safeIndex = Math.max(0, Math.min(size - 1, Math.trunc(index)));
    return layoutOffsetAt(layout, safeIndex);
}

export function computeAnchoredScrollTop(
    scrollTop: number,
    anchorTopBefore: number,
    anchorTopAfter: number,
): number {
    const safeScrollTop = finiteNonNegative(scrollTop, 0);
    const before = Number.isFinite(anchorTopBefore) ? anchorTopBefore : 0;
    const after = Number.isFinite(anchorTopAfter) ? anchorTopAfter : before;
    return Math.max(0, safeScrollTop + after - before);
}

export function findRetainedMessagePredecessorIndex(
    messageIds: readonly string[],
    anchorIndex: number,
    retainedMessageIds: ReadonlySet<string>,
): number | undefined {
    const start = Math.min(messageIds.length, Math.max(0, Math.trunc(anchorIndex))) - 1;
    for (let index = start; index >= 0; index -= 1) {
        const messageId = messageIds[index];
        if (messageId !== undefined && retainedMessageIds.has(messageId)) return index;
    }
    return undefined;
}

export function isVirtualMessageNearBottom(
    scrollHeight: number,
    scrollTop: number,
    viewportHeight: number,
    threshold = 120,
): boolean {
    const safeScrollHeight = finiteNonNegative(scrollHeight, 0);
    const safeScrollTop = finiteNonNegative(scrollTop, 0);
    const safeViewportHeight = finiteNonNegative(viewportHeight, 0);
    const safeThreshold = finiteNonNegative(threshold, 120);
    return safeScrollHeight - safeScrollTop - safeViewportHeight < safeThreshold;
}

export function virtualWindowContainsIndex(window: VirtualWindow, index: number): boolean {
    const safeIndex = Math.trunc(index);
    return safeIndex >= window.start && safeIndex < window.end;
}
