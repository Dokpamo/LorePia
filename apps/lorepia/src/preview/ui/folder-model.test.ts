import { describe, expect, it } from 'vitest';
import { extractCard, groupCards, syncRail } from './folder-model';

const folder = { id: 'folder-1', name: '친구들' };
const initial = () => syncRail([], ['a', 'b', 'c', 'd']);
const ids = (entries: ReturnType<typeof initial>) => entries.flatMap((entry) => entry.cards);

describe('sample card folders', () => {
    it('groups at the target position and preserves every character exactly once', () => {
        const entries = groupCards(initial(), 'a', 'b', folder);
        expect(entries.map((entry) => entry.id)).toEqual(['folder-1', 'c', 'd']);
        expect(entries[0]?.cards).toEqual(['b', 'a']);
        expect([...ids(entries)].sort()).toEqual(['a', 'b', 'c', 'd']);
        expect(initial()).toHaveLength(4);
    });
    it('adds a card to an existing named folder, then dissolves single-card remnants', () => {
        let entries = groupCards(initial(), 'a', 'b', folder);
        entries = groupCards(entries, 'c', 'folder-1', { id: 'ignored', name: 'ignored' });
        expect(entries[0]).toEqual({ ...folder, cards: ['b', 'a', 'c'] });
        entries = extractCard(extractCard(entries, 'a'), 'c');
        expect(entries.every((entry) => entry.cards.length === 1)).toBe(true);
        expect([...ids(entries)].sort()).toEqual(['a', 'b', 'c', 'd']);
    });
    it('moves one member between folders without moving its siblings', () => {
        let entries = groupCards(initial(), 'a', 'b', folder);
        entries = groupCards(entries, 'c', 'd', { id: 'folder-2', name: '다른 친구' });
        entries = groupCards(entries, 'a', 'folder-2', folder);
        expect(entries[0]?.cards).toEqual(['b']);
        expect(entries[1]?.cards).toEqual(['d', 'c', 'a']);
    });
    it('rejects self, missing and same-folder drops', () => {
        const entries = groupCards(initial(), 'a', 'b', folder);
        for (const [source, target] of [
            ['a', 'b'],
            ['a', 'a'],
            ['missing', 'c'],
        ] as const)
            expect(groupCards(entries, source, target, folder)).toBe(entries);
    });
    it('retains folders when a character is added and prunes removed or duplicate members', () => {
        const entries = groupCards(initial(), 'a', 'b', folder);
        const next = syncRail(
            [...entries, { id: 'duplicate', name: '', cards: ['a'] }],
            ['a', 'b', 'c', 'e'],
        );
        expect(next[0]).toEqual(entries[0]);
        expect(ids(next)).toEqual(['b', 'a', 'c', 'e']);
    });
});
