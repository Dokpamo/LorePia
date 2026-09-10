import { describe, expect, it, vi } from 'vitest';
import { ChatScrollLifecycle, type MessageCollectionSnapshot } from '../chat-scroll.svelte';
import { VIRTUAL_MESSAGE_DOM_LIMIT } from '../virtual-window';
import type { MessageDto } from '../../../lib/ipc/contracts';

describe('date group extension in a virtual transcript', () => {
    it.each([1_000, 100_000])('bounds date work for %i messages on the same day', (count) => {
        const day = vi.fn(() => '2026-09-08');
        const scroll = new ChatScrollLifecycle({
            currentCollection: () => collection,
            messageDayKey: day,
            onMemorySourceFocused: () => undefined,
            onMemorySourceMissing: () => undefined,
        });
        const items = Array.from(
            { length: count },
            (_, index) => ({ id: String(index), created_at: '2026-09-08T00:00:00Z' }) as MessageDto,
        );
        const collection: MessageCollectionSnapshot = scroll.snapshotMessageCollection(items);
        scroll.syncCollection(collection);
        scroll.applyProgrammaticScrollPosition(document.createElement('div'), count * 108 - 1000);
        day.mockClear();
        const window = scroll.virtualWindow();
        expect(window.end - window.start).toBeLessThanOrEqual(VIRTUAL_MESSAGE_DOM_LIMIT);
        expect(window.start).toBeGreaterThan(count - 100);
        expect(day.mock.calls.length).toBeLessThanOrEqual(VIRTUAL_MESSAGE_DOM_LIMIT);
    });
});
