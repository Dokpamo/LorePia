import { describe, expect, it, vi } from 'vitest';
import type { MessageDto } from '../../lib/ipc/contracts';
import type {
    BranchMessagesPageDto,
    ListBranchMessagesPageInput,
} from '../../lib/ipc/contracts/message-history';
import {
    loadRecentBranchMessages,
    messageWindowMetadata,
} from '../controllers/recent-branch-messages';
import { createAppControllerFixture } from './app-controller-test-support';

function fixture() {
    const { conversation, conversationState, mockClient } = createAppControllerFixture();
    let snapshot = 'opaque-snapshot-one';
    let transferredBodies = 0;
    const messages = Array.from({ length: 200 }, (_, index): MessageDto => ({
        id: `m${String(index)}`,
        parent_id: index ? `m${String(index - 1)}` : null,
        conversation_id: conversation.id,
        role: index % 2 ? 'assistant' : 'user',
        content: `content-${String(index)}`,
        status: 'complete',
        generation_id: null,
        created_at: '2026-09-12T00:00:00Z',
    }));
    const read = (input: ListBranchMessagesPageInput): Promise<BranchMessagesPageDto> => {
        const end = input.before_message_id
            ? messages.findIndex((message) => message.id === input.before_message_id)
            : messages.length;
        if (end < 0) throw Object.assign(new Error('removed anchor'), { code: 'invalid_input' });
        const start = Math.max(0, end - input.limit);
        const selected = messages.slice(start, end).map((message) => ({ ...message }));
        const assistant = input.include_last_assistant
            ? [...messages].reverse().find((message) => message.role === 'assistant')
            : null;
        transferredBodies += selected.length;
        return Promise.resolve({
            messages: selected,
            snapshot_token: snapshot,
            head_message_id: messages.at(-1)?.id ?? null,
            start_index: start,
            total_messages: messages.length,
            has_older: start > 0,
            has_newer: end < messages.length,
            ...(assistant && !selected.some((message) => message.id === assistant.id)
                ? { last_assistant_message: { ...assistant } }
                : {}),
        });
    };
    const page = vi.fn(read);
    const client = { ...mockClient({}), listBranchMessagesPage: page };
    const load = (initial: (messages: MessageDto[]) => boolean | undefined = vi.fn()) =>
        loadRecentBranchMessages(client, conversationState.active_branch_id, initial);
    return {
        messages,
        page,
        read,
        load,
        transferredBodies: () => transferredBodies,
        changeSnapshot: () => {
            snapshot = 'opaque-snapshot-two';
        },
    };
}

describe('recent-history snapshot reuse', () => {
    it('paints 30 and transfers only the remaining 98 bodies for an unchanged branch', async () => {
        const { load, messages, page, transferredBodies } = fixture();
        const initial = vi.fn();
        const result = await load(initial);
        expect(initial).toHaveBeenCalledWith(messages.slice(-30));
        expect(result).toEqual(messages.slice(-128));
        expect(page.mock.calls.map(([input]) => input.limit)).toEqual([30, 98]);
        expect(page.mock.calls.at(1)?.[0]).toMatchObject({
            before_message_id: 'm170',
            include_last_assistant: false,
        });
        expect(transferredBodies()).toBe(128);
        expect(messageWindowMetadata(result)).toMatchObject({
            start_index: 72,
            total_messages: 200,
            head_message_id: 'm199',
        });
    });

    it.each(['body', 'status', 'checkpoint', 'projection'])(
        'rejects a changed %s snapshot with unchanged identities and positions',
        async (kind) => {
            const { load, messages, page, changeSnapshot } = fixture();
            const initial = vi.fn((initialMessages: MessageDto[]) => {
                expect(initialMessages).toHaveLength(30);
                const last = messages.at(-1);
                if (!last) throw new Error('missing fixture message');
                if (kind === 'status') last.status = 'cancelled';
                else last.content = `changed-${kind}`;
                changeSnapshot();
                return undefined;
            });
            const result = await load(initial);
            expect(page.mock.calls.map(([input]) => input.limit)).toEqual([30, 98, 128]);
            expect(result).toEqual(messages.slice(-128));
            expect(result.at(-1)).not.toEqual(initial.mock.calls.at(0)?.[0].at(-1));
        },
    );

    it('falls back when removal invalidates the first page anchor', async () => {
        const { load, messages, page, changeSnapshot } = fixture();
        const result = await load(
            vi.fn(() => {
                messages.splice(170);
                changeSnapshot();
                return undefined;
            }),
        );
        expect(page.mock.calls.map(([input]) => input.limit)).toEqual([30, 98, 128]);
        expect(result).toEqual(messages.slice(-128));
    });

    it('keeps the bounded compatibility path when a peer provides no token', async () => {
        const { load, page, read } = fixture();
        page.mockImplementation(async (input) => ({
            ...(await read(input)),
            snapshot_token: undefined,
        }));
        expect(await load()).toHaveLength(128);
        expect(page.mock.calls.map(([input]) => input.limit)).toEqual([30, 128]);
    });

    it('rejects nonadjacent results even when their token matches', async () => {
        const { load, page, read, messages } = fixture();
        page.mockImplementationOnce(read).mockImplementationOnce(async (input) => ({
            ...(await read(input)),
            start_index: 71,
        }));
        expect(await load()).toEqual(messages.slice(-128));
        expect(page.mock.calls.map(([input]) => input.limit)).toEqual([30, 98, 128]);
    });

    it('retains one distant assistant separately when both page halves lack an assistant', async () => {
        const { load, page, messages } = fixture();
        for (const message of messages) message.role = 'user';
        const assistant = messages.at(50);
        if (!assistant) throw new Error('missing fixture assistant');
        assistant.role = 'assistant';
        const result = await load();
        expect(page.mock.calls.at(1)?.[0].include_last_assistant).toBe(true);
        expect(result).toHaveLength(128);
        expect(messageWindowMetadata(result).last_assistant_message).toEqual(messages[50]);
        expect(result.some((message) => message.id === 'm50')).toBe(false);
    });

    it('does not suppress integrity or transport errors with an extra read', async () => {
        const { load, page, read } = fixture();
        const error = { code: 'storage_corrupted' };
        page.mockImplementationOnce(read).mockRejectedValueOnce(error);
        await expect(load()).rejects.toBe(error);
        expect(page).toHaveBeenCalledTimes(2);
    });
});
