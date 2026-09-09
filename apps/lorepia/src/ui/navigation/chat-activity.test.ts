import { describe, expect, it } from 'vitest';
import { chatActivityTime, latestChatTimes } from './chat-activity';
import type { ConversationListItem } from './navigation-types';

describe('last chat presentation', () => {
    it('uses the newest valid chat per character without modifying catalog order', () => {
        const items = [
            { id: 'c', characterId: 'one', updatedAt: '2026-09-09T09:00:00Z' },
            { id: 'a', characterId: 'one', updatedAt: '2026-09-08T10:00:00Z' },
            { id: 'b', characterId: 'two', updatedAt: '2026-09-07T09:00:00Z' },
            { id: 'invalid', characterId: 'one', updatedAt: 'invalid' },
        ].map(
            (item) =>
                ({
                    title: '',
                    date: '',
                    characterName: '',
                    ...item,
                }) satisfies ConversationListItem,
        );
        expect([...latestChatTimes(items)]).toEqual([
            ['one', items[0]?.updatedAt],
            ['two', items[2]?.updatedAt],
        ]);
        expect(items.map((item) => item.id)).toEqual(['c', 'a', 'b', 'invalid']);
    });
    it('shows time today, date on earlier days and year for old chats, with a full timestamp', () => {
        const now = new Date(2026, 8, 9, 15, 0);
        const today = chatActivityTime(new Date(2026, 8, 9, 13, 4).toISOString(), 'en', now);
        expect(today?.short).toMatch(/1:04/);
        expect(today?.full).toContain('2026');
        expect(chatActivityTime(new Date(2026, 8, 8).toISOString(), 'en', now)?.short).toBe('9/8');
        expect(chatActivityTime(new Date(2025, 8, 8).toISOString(), 'en', now)?.short).toContain(
            '2025',
        );
        expect(chatActivityTime('invalid', 'ko', now)).toBeNull();
    });
});
