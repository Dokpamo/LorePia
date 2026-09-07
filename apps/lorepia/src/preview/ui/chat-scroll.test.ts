import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { MessageDto } from '../../lib/ipc/contracts';
import {
    ChatScrollLifecycle,
    type MessageCollectionSnapshot,
} from '../../features/chat/chat-scroll.svelte';

class MeasuredObserver {
    static all: MeasuredObserver[] = [];
    nodes = new Set<Element>();
    constructor(readonly callback: ResizeObserverCallback) {
        MeasuredObserver.all.push(this);
    }
    observe(node: Element) {
        this.nodes.add(node);
    }
    unobserve(node: Element) {
        this.nodes.delete(node);
    }
    disconnect() {
        this.nodes.clear();
    }
    static emit(node: Element, width: number, height: number) {
        for (const observer of MeasuredObserver.all)
            if (observer.nodes.has(node))
                observer.callback(
                    [
                        {
                            target: node,
                            contentRect: DOMRect.fromRect({ width, height }),
                            borderBoxSize: [{ inlineSize: width, blockSize: height }],
                            contentBoxSize: [],
                            devicePixelContentBoxSize: [],
                        },
                    ],
                    observer,
                );
    }
}
afterEach(() => {
    document.body.replaceChildren();
    vi.unstubAllGlobals();
    MeasuredObserver.all = [];
});

async function measuredWindow(scale: number) {
    vi.stubGlobal('ResizeObserver', MeasuredObserver);
    const items: MessageDto[] = Array.from({ length: 300 }, (_, index) => ({
        id: 'message-' + String(index),
        conversation_id: 'sample',
        parent_id: null,
        role: 'assistant',
        content: '',
        status: 'complete',
        generation_id: null,
        created_at: '2026-09-07T00:00:00Z',
    }));
    const scroll: ChatScrollLifecycle = new ChatScrollLifecycle({
        currentCollection: () => collection,
        messageDayKey: () => 'day',
        onMemorySourceMissing: vi.fn(),
        onMemorySourceFocused: vi.fn(),
    });
    const collection: MessageCollectionSnapshot = scroll.snapshotMessageCollection(items);
    const log = document.createElement('div');
    document.body.append(log);
    Object.defineProperties(log, {
        clientHeight: { value: 600 },
        scrollHeight: { value: 60_000 },
        offsetWidth: { value: 360 },
        getBoundingClientRect: {
            value: () => DOMRect.fromRect({ width: 360 * scale, height: 600 * scale }),
        },
    });
    scroll.scroller = log;
    scroll.syncBranch('sample', vi.fn());
    scroll.syncCollection(collection);
    await tick();
    scroll.applyProgrammaticScrollPosition(log, 1);
    const dispose = scroll.observeScroller();
    MeasuredObserver.emit(log, 360, 600);
    const actions = [];
    for (let index = 0; index < 80; index++) {
        const message = items[index];
        if (!message) throw new Error('Missing measured row');
        const row = document.createElement('article');
        row.dataset.messageId = message.id;
        row.getBoundingClientRect = () =>
            DOMRect.fromRect({
                width: 320 * scale,
                height: 180 * scale,
                y: (22 + index * 192 - log.scrollTop) * scale,
            });
        log.append(row);
        actions.push(
            scroll.measureMessage(row, {
                messageId: message.id,
                epoch: scroll.measurementEpoch,
                includesDayDivider: false,
            }),
        );
        MeasuredObserver.emit(row, 320, 180);
    }
    await tick();
    await tick();
    scroll.applyProgrammaticScrollPosition(log, 18000);
    const window = scroll.virtualWindow();
    actions.forEach((action) => action.destroy());
    dispose?.();
    log.remove();
    return window;
}
describe('original scroll owner in a scaled preview', () => {
    it('keeps virtual message sizes and offsets in CSS pixels below the mobile design width', async () => {
        const normal = await measuredWindow(1);
        const scaled = await measuredWindow(320 / 360);
        expect(scaled).toEqual(normal);
        expect(scaled.start).toBeGreaterThan(0);
        expect(scaled.end - scaled.start).toBeLessThanOrEqual(80);
    });
});
