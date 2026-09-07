import { afterEach, describe, expect, it, vi } from 'vitest';
import { t } from '../../lib/i18n';
import { createSampleCharacters, type SampleConversation } from './sample-data';
import { SampleChatSession } from './sample-chat.svelte';

const sessions: SampleChatSession[] = [];
afterEach(() => {
    sessions.splice(0).forEach((session) => session.dispose());
    vi.useRealTimers();
});
function setup() {
    vi.useFakeTimers();
    const first = createSampleCharacters()[0]?.histories[0];
    if (!first) throw new Error('Missing sample');
    let current: SampleConversation = first;
    const session = new SampleChatSession(() => current);
    sessions.push(session);
    return {
        first,
        session,
        switchTo: (next: SampleConversation) => {
            current = next;
        },
    };
}
describe('preview chat branches and response lifecycle', () => {
    it('does not create a branch for blank edits or apply an editor from another source branch', () => {
        const { first, session } = setup();
        const change = session.edit('a1-m2');
        change(' \n ');
        expect(first.branches).toBeUndefined();
        session.fork('a1-m3');
        const before = structuredClone(first.messages);
        change('stale edit from the original branch');
        expect(first.messages).toEqual(before);
        expect(first.branches).toHaveLength(2);
    });
    it('edits into one branch while preserving the original message and its continuation', () => {
        const { first, session } = setup();
        const before = structuredClone(first.messages);
        const change = session.edit('a1-m2');
        change('changed once');
        const branch = first.activeBranchId;
        change('changed twice');
        expect(first.branches).toHaveLength(2);
        expect(first.messages.at(-1)?.text).toBe('changed twice');
        expect(first.messages).toHaveLength(2);
        expect(first.messages.at(-1)?.id).not.toBe('a1-m2');
        session.selectBranch('main');
        expect(first.messages).toEqual(before);
        change('stale editor');
        expect(first.messages).toEqual(before);
        session.selectBranch(branch ?? '');
        expect(first.messages.at(-1)?.text).toBe('changed twice');
    });
    it('shows a streamed sample, rejects duplicate sends, and stops without losing partial text', () => {
        const { first, session } = setup();
        expect(session.send('hello')).toBe(true);
        expect(session.send('duplicate')).toBe(false);
        vi.advanceTimersByTime(350);
        const response = first.messages.at(-1);
        expect(response?.text.length).toBeGreaterThan(0);
        session.stop();
        const partial = response?.text;
        vi.runAllTimers();
        expect(response?.status).toBe('cancelled');
        expect(response?.text).toBe(partial);
        expect(session.busy).toBe(false);
        expect(vi.getTimerCount()).toBe(0);
    });
    it('can retry a failed response in a new branch and still return to the failed original', () => {
        const { first, session } = setup();
        first.responsePreview = 'failed';
        session.send('hello');
        vi.runAllTimers();
        const failed = first.messages.at(-1);
        expect(failed?.status).toBe('failed');
        session.regenerate(failed?.id ?? '', true);
        vi.runAllTimers();
        expect(first.messages.at(-1)?.status).toBe('complete');
        expect(first.messages.at(-1)?.text).toBe(t('uiPreview.sampleReply'));
        session.selectBranch('main');
        expect(first.messages.at(-1)).toBe(failed);
    });
    it('cancels an old room response before any update can reach the next room', () => {
        const { first, session, switchTo } = setup();
        session.send('hello');
        const next = { id: 'next', title: '', date: '', messages: [] };
        switchTo(next);
        vi.runAllTimers();
        expect(first.messages.at(-1)?.status).toBe('cancelled');
        expect(first.messages.at(-1)?.text).toBe('');
        expect(next.messages).toEqual([]);
        expect(session.busy).toBe(false);
    });
    it('keeps the original assistant response when regenerating in story mode', () => {
        const { first, session } = setup();
        const before = structuredClone(first.messages);
        first.mode = 'story';
        session.regenerate('a1-m3');
        vi.runAllTimers();
        expect(first.messages.at(-1)?.text).toBe(t('uiPreview.sampleStory'));
        session.selectBranch('main');
        expect(first.messages).toEqual(before);
    });
});
