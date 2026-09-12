import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';
import { MessageHistoryController } from '../controllers/message-history-controller';
import {
    loadRecentBranchMessages,
    messageWindowMetadata,
    type HistoryPageClient,
} from '../controllers/recent-branch-messages';
import { INITIAL_APP_STATE, type LorepiaAppState } from '../app-state';
import { createAppControllerFixture, deferred } from './app-controller-test-support';
import type { MessageDto } from '../../lib/ipc/contracts';
const { conversation, conversationState, mockClient } = createAppControllerFixture();
const messages = Array.from({ length: 2000 }, (_, index): MessageDto => ({
    id: `m-${String(index)}`,
    parent_id: index ? `m-${String(index - 1)}` : null,
    conversation_id: conversation.id,
    role: index % 2 ? 'assistant' : 'user',
    content: `text-${String(index)}`,
    status: 'complete',
    generation_id: null,
    created_at: '2026-09-12T00:00:00Z',
}));
function fixture() {
    const page = vi.fn(
        (input: {
            before_message_id?: string | null;
            after_message_id?: string | null;
            limit: number;
        }) => {
            const before = input.before_message_id
                ? messages.findIndex((item) => item.id === input.before_message_id)
                : messages.length;
            const after = input.after_message_id
                ? messages.findIndex((item) => item.id === input.after_message_id) + 1
                : null;
            const start = after ?? Math.max(0, before - input.limit);
            const end = after === null ? before : Math.min(messages.length, start + input.limit);
            return Promise.resolve({
                messages: messages.slice(start, end),
                start_index: start,
                total_messages: messages.length,
                head_message_id: 'm-1999',
                has_older: start > 0,
                has_newer: end < messages.length,
            });
        },
    );
    const client: HistoryPageClient = { ...mockClient({}), listBranchMessagesPage: page };
    const history = new MessageHistoryController(client);
    const app: LorepiaAppState = {
        ...structuredClone(INITIAL_APP_STATE),
        selected_conversation: conversation,
        conversation_state: conversationState,
        messages: {
            phase: 'ready',
            error: null,
            items: messages.slice(-128),
            start_index: 1872,
            total_messages: 2000,
            head_message_id: 'm-1999',
        },
    };
    history.sync(app);
    return { page, client, history, app };
}
describe('bounded chat history', () => {
    it('paints 30 first, keeps runtime at 128, and pages both directions within 90 retained messages', async () => {
        const { client, history, page } = fixture();
        expect(get(history.state).items).toEqual(messages.slice(-30));
        const initial = vi.fn();
        const recent = await loadRecentBranchMessages(
            client,
            conversationState.active_branch_id,
            initial,
        );
        expect(initial).toHaveBeenCalledWith(messages.slice(-30));
        expect(recent).toHaveLength(128);
        expect(messageWindowMetadata(recent)).toEqual({
            start_index: 1872,
            total_messages: 2000,
            head_message_id: 'm-1999',
        });
        expect(page.mock.calls.map(([input]) => input.limit)).toEqual([30, 128]);
        for (let index = 0; index < 10; index += 1) {
            await history.load('older');
            expect(get(history.state).items.length).toBeLessThanOrEqual(90);
        }
        expect(get(history.state).items).toEqual(messages.slice(1670, 1760));
        expect(get(history.state).has_newer).toBe(true);
        await history.load('newer');
        expect(get(history.state).items).toEqual(messages.slice(1700, 1790));
        await history.load('latest');
        expect(get(history.state).items).toEqual(messages.slice(-30));
        history.dispose();
        expect(get(history.state).items).toEqual([]);
    });
    it('ignores a late page after switching branches', async () => {
        const { history, page, app } = fixture();
        const delayed = deferred<Awaited<ReturnType<typeof page>>>();
        page.mockReturnValueOnce(delayed.promise);
        const loading = history.load('older');
        history.sync({
            ...app,
            conversation_state: { ...conversationState, active_branch_id: 'next' },
            messages: { phase: 'loading', error: null, items: [] },
        });
        delayed.resolve({
            messages: messages.slice(1940, 1970),
            start_index: 1940,
            total_messages: 2000,
            head_message_id: 'm-1999',
            has_older: true,
            has_newer: true,
        });
        await loading;
        expect(get(history.state).items).toEqual([]);
        expect(get(history.state).scope).toContain('next');
        history.dispose();
    });
    it('keeps older viewport on head extension and discards incompatible page snapshots', async () => {
        const { history, app, page } = fixture();
        for (let index = 0; index < 3; index += 1) await history.load('older');
        const oldItems = get(history.state).items;
        history.sync({
            ...app,
            messages: {
                ...app.messages,
                items: [...app.messages.items],
                total_messages: 2002,
                head_message_id: 'new-head',
            },
        });
        expect(get(history.state).items).toBe(oldItems);
        await history.load('older');
        expect(page.mock.calls.at(-1)?.[0]).toEqual({
            branch_id: conversationState.active_branch_id,
            limit: 30,
        });
        expect(get(history.state).items).toEqual(messages.slice(-30));
        history.dispose();
    });
    it('waits for the complete recent context after publishing the initial page', async () => {
        const { client, page } = fixture();
        const delayed = deferred<Awaited<ReturnType<typeof page>>>();
        const original = page.getMockImplementation();
        if (!original) throw new Error('fixture missing');
        page.mockImplementationOnce(original).mockReturnValueOnce(delayed.promise);
        const initial = vi.fn();
        let ready = false;
        const loading = loadRecentBranchMessages(
            client,
            conversationState.active_branch_id,
            initial,
        ).then((value) => {
            ready = true;
            return value;
        });
        await vi.waitFor(() => expect(initial).toHaveBeenCalledOnce());
        expect(ready).toBe(false);
        delayed.resolve(await original({ limit: 128 }));
        expect(await loading).toHaveLength(128);
    });
});

it('coalesces scroll and latest requests while one native page is pending', async () => {
    const { history, page } = fixture();
    const delayed = deferred<Awaited<ReturnType<typeof page>>>();
    page.mockReturnValueOnce(delayed.promise);
    const older = history.load('older');
    const latest = history.load('latest');
    const repeated = Array.from({ length: 20 }, () => history.load('latest'));
    expect(page).toHaveBeenCalledOnce();
    delayed.resolve({
        messages: messages.slice(1940, 1970),
        start_index: 1940,
        total_messages: 2000,
        head_message_id: 'm-1999',
        has_older: true,
        has_newer: true,
    });
    await Promise.all([older, latest, ...repeated]);
    expect(page).toHaveBeenCalledTimes(2);
    expect(get(history.state).items).toEqual(messages.slice(-30));
    history.dispose();
});

it('keeps the visible window on failure and permits an explicit retry', async () => {
    const { history, page } = fixture();
    const before = get(history.state).items;
    page.mockRejectedValueOnce(new Error('temporary failure'));
    await history.load('older');
    expect(get(history.state).items).toBe(before);
    expect(get(history.state).failed_direction).toBe('older');
    await history.load('older');
    expect(get(history.state).items).toHaveLength(60);
    expect(get(history.state).error).toBeNull();
    history.dispose();
});

it('requests one older assistant aggregate only with the final runtime context and never adds it to the window', async () => {
    const { client, page } = fixture();
    const older = { ...messages[0], id: 'older-assistant', role: 'assistant' } as MessageDto;
    const original = page.getMockImplementation();
    if (!original) throw new Error('Missing page fixture');
    page.mockImplementation(async (input) => ({
        ...(await original(input)),
        ...(input.limit === 128 ? { last_assistant_message: older } : {}),
    }));
    const recent = await loadRecentBranchMessages(
        client,
        conversationState.active_branch_id,
        vi.fn(),
    );
    expect(page.mock.calls.map(([input]) => input)).toEqual([
        { branch_id: conversationState.active_branch_id, limit: 30 },
        { branch_id: conversationState.active_branch_id, limit: 128, include_last_assistant: true },
    ]);
    expect(recent).toHaveLength(128);
    expect(recent.some((message) => message.id === older.id)).toBe(false);
    expect(messageWindowMetadata(recent).last_assistant_message).toEqual(older);
});
