import { describe, expect, it } from 'vitest';

import {
    VIRTUAL_MESSAGE_DOM_LIMIT,
    VirtualMessageLayoutIndex,
    VirtualMessageMeasurements,
    buildVirtualMessageLayout,
    computeAnchoredScrollTop,
    computeVirtualMessageWindow,
    findRetainedMessagePredecessorIndex,
    isVirtualMessageNearBottom,
    virtualWindowContainsIndex,
} from './virtual-window';

describe('variable-height message virtualization', () => {
    it('uses measured heights and row gaps for offsets and binary-search windows', () => {
        const ids = Array.from({ length: 120 }, (_, index) => `message-${String(index)}`);
        const measurements = new Map<string, number>([
            ['message-0', 48],
            ['message-1', 240],
            ['message-2', 72],
            ['message-19', 360],
            ['message-20', 144],
        ]);
        const layout = buildVirtualMessageLayout(ids, measurements);
        const indexedLayout = new VirtualMessageLayoutIndex(ids, measurements);

        expect(layout.offsets.slice(0, 4)).toEqual([0, 60, 312, 396]);
        expect(layout.offsets[20]).toBe(2_496);
        expect([0, 1, 2, 3].map((index) => indexedLayout.offsetAt(index))).toEqual([
            0, 60, 312, 396,
        ]);
        expect(indexedLayout.offsetAt(20)).toBe(2_496);
        expect(indexedLayout.totalHeight).toBe(layout.totalHeight);

        const window = computeVirtualMessageWindow(layout, layout.offsets[20] ?? 0, 96);
        expect(window.start).toBe(12);
        expect(window.end).toBe(29);
        expect(window.topSpacer).toBe(layout.offsets[12]);
        expect(window.bottomSpacer).toBe(
            layout.totalHeight -
                ((layout.offsets[window.end - 1] ?? 0) + (layout.heights[window.end - 1] ?? 0)),
        );
        expect(computeVirtualMessageWindow(indexedLayout, indexedLayout.offsetAt(20), 96)).toEqual(
            window,
        );
    });

    it('keeps the rendered window capped at 80 even for a dense viewport', () => {
        const ids = Array.from({ length: 10_000 }, (_, index) => `message-${String(index)}`);
        const measurements = new Map(ids.map((id) => [id, 1] as const));
        const layout = buildVirtualMessageLayout(ids, measurements, { gap: 0 });

        const window = computeVirtualMessageWindow(layout, 0, 10_000);

        expect(window.start).toBe(0);
        expect(window.end - window.start).toBe(VIRTUAL_MESSAGE_DOM_LIMIT);
    });

    it('invalidates measurements on width or room changes and rejects stale epochs', () => {
        const measurements = new VirtualMessageMeasurements();
        const branchOneEpoch = measurements.resetScope('conversation-1:branch-1');
        measurements.setViewportWidth(900);

        expect(measurements.record(branchOneEpoch, 'message-1', 144)).toBe(true);
        expect(measurements.values.get('message-1')).toBe(144);

        const narrowEpoch = measurements.setViewportWidth(520);
        expect(narrowEpoch).not.toBe(branchOneEpoch);
        expect(measurements.values.size).toBe(0);
        expect(measurements.record(branchOneEpoch, 'message-stale-width', 200)).toBe(false);
        expect(measurements.record(narrowEpoch, 'message-1', 220)).toBe(true);

        const branchTwoEpoch = measurements.resetScope('conversation-1:branch-2');
        expect(branchTwoEpoch).not.toBe(narrowEpoch);
        expect(measurements.values.size).toBe(0);
        expect(measurements.record(narrowEpoch, 'message-stale-branch', 240)).toBe(false);
    });

    it('preserves the visible anchor when measurements above it change', () => {
        expect(computeAnchoredScrollTop(1_200, 180, 324)).toBe(1_344);
        expect(computeAnchoredScrollTop(40, 120, 20)).toBe(0);
    });

    it('updates one of 10,000 measured rows without rescanning the message collection', () => {
        let numericIndexReads = 0;
        const sourceIds = Array.from({ length: 10_000 }, (_, index) => `message-${String(index)}`);
        const ids = new Proxy(sourceIds, {
            get(target, property, receiver) {
                if (typeof property === 'string' && /^\d+$/.test(property)) {
                    numericIndexReads += 1;
                }
                return Reflect.get(target, property, receiver) as unknown;
            },
        });
        const layout = new VirtualMessageLayoutIndex(ids);
        numericIndexReads = 0;
        const before = layout.offsetAt(9_001);

        expect(layout.updateMeasuredHeight('message-9000', 480)).toBe(true);
        expect(layout.offsetAt(9_001) - before).toBe(384);
        expect(
            computeVirtualMessageWindow(layout, layout.offsetAt(9_000), 720).start,
        ).toBeGreaterThan(8_900);
        expect(numericIndexReads).toBe(0);
    });

    it('selects the nearest retained pre-delete predecessor as an anchor fallback', () => {
        const ids = ['message-0', 'message-1', 'message-2', 'message-3', 'message-4'];

        expect(
            findRetainedMessagePredecessorIndex(ids, 3, new Set(['message-0', 'message-1'])),
        ).toBe(1);
        expect(findRetainedMessagePredecessorIndex(ids, 1, new Set(['message-2']))).toBeUndefined();
    });

    it('computes bottom proximity from current viewport metrics and window membership', () => {
        expect(isVirtualMessageNearBottom(2_000, 1_000, 720)).toBe(false);
        expect(isVirtualMessageNearBottom(2_000, 1_200, 720)).toBe(true);
        expect(
            virtualWindowContainsIndex({ start: 10, end: 30, topSpacer: 0, bottomSpacer: 0 }, 29),
        ).toBe(true);
        expect(
            virtualWindowContainsIndex({ start: 10, end: 30, topSpacer: 0, bottomSpacer: 0 }, 30),
        ).toBe(false);
    });
});
