import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';
import type { ChatStreamItemDto, MessageDto } from '../../lib/ipc/contracts';
import { LorepiaAppController } from '../app-controller';
import { mergeTerminalMessages } from '../controllers/terminal-message-refresh';
import { createAppControllerFixture } from './app-controller-test-support';

const { character, conversation, conversationState, branch, mockClient, terminalEvent } =
    createAppControllerFixture();
const user: MessageDto = {
    id: 'user-new',
    conversation_id: conversation.id,
    parent_id: null,
    role: 'user',
    status: 'complete',
    content: 'new question',
    generation_id: null,
    created_at: '2026-08-02T00:00:00Z',
};
const assistant: MessageDto = {
    ...user,
    id: 'message-1',
    parent_id: user.id,
    role: 'assistant',
    generation_id: 'generation-1',
    content: 'verified display',
};

async function runTerminal(
    existing: MessageDto[],
    pair: MessageDto[],
    reason: 'terminal' | 'gap' = 'terminal',
) {
    const listeners: ((item: ChatStreamItemDto) => void)[] = [];
    const listGenerationMessages = vi.fn().mockResolvedValue(pair);
    const listBranchMessages = vi.fn().mockResolvedValueOnce(existing).mockResolvedValue(pair);
    const listBranches = vi
        .fn()
        .mockResolvedValueOnce([{ ...branch, head_message_id: existing.at(-1)?.id ?? null }])
        .mockResolvedValue([{ ...branch, head_message_id: assistant.id }]);
    const controller = new LorepiaAppController(
        mockClient({
            listGenerationMessages,
            listBranchMessages,
            listBranches,
            sendMessage: (_input, _streamId, listener) => {
                listeners.push(listener);
                return Promise.resolve({ generation_id: 'generation-1' });
            },
        }),
    );
    await controller.start();
    await controller.selectCharacter(character);
    await controller.selectConversation(conversation);
    await controller.sendMessage('new question');
    listeners[0]?.(
        reason === 'terminal'
            ? terminalEvent(1)
            : {
                  type: 'reconciliation_required',
                  payload: {
                      reason: 'broadcast_lagged',
                      generation_id: 'generation-1',
                      conversation_id: conversation.id,
                      branch_id: branch.id,
                      last_sequence: 1,
                      observed_sequence: null,
                      dropped_events: 1,
                      supported_event_version: 4,
                      display_prefix: null,
                      reasoning_prefix: null,
                  },
              },
    );
    await vi.waitFor(() => expect(get(controller.state).chat.phase).toBe('idle'));
    return { controller, listGenerationMessages, listBranchMessages };
}

describe('verified terminal incremental history refresh', () => {
    it('reads only the terminal pair for a long existing transcript and preserves prior items', async () => {
        const history = Array.from({ length: 10_000 }, (_, index): MessageDto => ({
            ...user,
            id: `old-${String(index)}`,
            parent_id: index ? `old-${String(index - 1)}` : null,
        }));
        const pair = [{ ...user, parent_id: history.at(-1)?.id ?? null }, assistant];
        const result = await runTerminal(history, pair);
        expect(result.listGenerationMessages).toHaveBeenCalledWith(
            conversation.id,
            conversationState.active_branch_id,
            'generation-1',
        );
        expect(result.listBranchMessages).toHaveBeenCalledTimes(1);
        const items = get(result.controller.state).messages.items;
        expect(items).toHaveLength(10_002);
        expect(items[0]).toBe(history[0]);
        expect(items.slice(-2)).toEqual(pair);
        result.controller.destroy();
    });

    it('falls back to full verified history when the returned suffix is discontinuous', async () => {
        const result = await runTerminal([], [{ ...user, parent_id: 'missing' }, assistant]);
        expect(result.listGenerationMessages).toHaveBeenCalledTimes(1);
        expect(result.listBranchMessages).toHaveBeenCalledTimes(2);
        result.controller.destroy();
    });

    it('keeps complete recovery for sequence gaps even when the incremental API exists', async () => {
        const result = await runTerminal([], [user, assistant], 'gap');
        expect(result.listGenerationMessages).not.toHaveBeenCalled();
        expect(result.listBranchMessages).toHaveBeenCalledTimes(2);
        result.controller.destroy();
    });

    it('replaces a pending pair and rejects wrong generation, head, route, or nonterminal data', () => {
        expect(
            mergeTerminalMessages(
                [user, { ...assistant, status: 'pending' }],
                [user, assistant],
                conversation.id,
                'generation-1',
                assistant.id,
            ),
        ).toEqual([user, assistant]);
        for (const patch of [
            { generation_id: 'other' },
            { id: 'other' },
            { conversation_id: 'other' },
            { status: 'pending' as const },
            { parent_id: 'other' },
        ]) {
            expect(
                mergeTerminalMessages(
                    [],
                    [user, { ...assistant, ...patch }],
                    conversation.id,
                    'generation-1',
                    assistant.id,
                ),
            ).toBeNull();
        }
        expect(mergeTerminalMessages([], [], conversation.id, 'generation-1', null)).toBeNull();
    });
});
