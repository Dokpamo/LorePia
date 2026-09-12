import { get } from 'svelte/store';
import { expect, it, vi } from 'vitest';
import type { ConversationStateDto, MessageDto } from '../../lib/ipc/contracts';
import type {
    BranchMessagesPageDto,
    ListBranchMessagesPageInput,
} from '../../lib/ipc/contracts/message-history';
import { LorepiaAppController } from '../app-controller';
import { createAppControllerFixture, deferred } from './app-controller-test-support';

const { conversation, conversationState, mockClient } = createAppControllerFixture();
const selected = (branch: string): ConversationStateDto => ({
    ...conversationState,
    active_branch_id: branch,
});
function page(branch: string, limit: number): BranchMessagesPageDto {
    return {
        messages: Array.from({ length: limit }, (_, index): MessageDto => ({
            id: `${branch}-${String(2000 - limit + index)}`,
            conversation_id: conversation.id,
            parent_id: null,
            role: 'user',
            content: 'text',
            status: 'complete',
            generation_id: null,
            created_at: conversation.created_at,
        })),
        start_index: 2000 - limit,
        total_messages: 2000,
        head_message_id: `${branch}-1999`,
        has_older: true,
        has_newer: false,
    };
}
async function fixture(stage: 'selection' | 'first' | 'context') {
    const selection = deferred<ConversationStateDto>();
    const first = deferred<BranchMessagesPageDto>();
    const context = deferred<BranchMessagesPageDto>();
    const pages = vi.fn((input: ListBranchMessagesPageInput) => {
        if (input.branch_id === conversationState.active_branch_id)
            return Promise.resolve({
                ...page(input.branch_id, 0),
                start_index: 0,
                total_messages: 0,
                has_older: false,
            });
        if (input.branch_id === 'old') {
            if (input.limit === 30 && stage === 'first') return first.promise;
            if (input.limit === 128 && stage === 'context') return context.promise;
        }
        return Promise.resolve(page(input.branch_id, input.limit));
    });
    const controller = new LorepiaAppController(
        mockClient({
            listBranchMessagesPage: pages,
            selectBranch: (_id, branch) =>
                branch === 'old' && stage === 'selection'
                    ? selection.promise
                    : Promise.resolve(selected(branch)),
        }),
    );
    await controller.selectConversation(conversation);
    pages.mockClear();
    return { controller, pages, selection, first, context };
}

it('publishes the selected branch first30 while its bounded runtime context is still loading', async () => {
    const { controller, pages, context } = await fixture('context');
    const loading = controller.selectBranch('old');
    await vi.waitFor(() => expect(get(controller.state).messages.items).toHaveLength(30));
    expect(get(controller.state)).toMatchObject({
        conversation_state: selected('old'),
        messages: { phase: 'loading', start_index: 1970, total_messages: 2000 },
    });
    expect(pages.mock.calls.map(([input]) => input.limit)).toEqual([30, 128]);
    context.resolve(page('old', 128));
    await loading;
    expect(get(controller.state).messages).toMatchObject({ phase: 'ready', start_index: 1872 });
    expect(get(controller.state).messages.items).toHaveLength(128);
    controller.destroy();
});

it.each(['selection', 'first', 'context'] as const)(
    'ignores an obsolete branch at the %s boundary',
    async (stage) => {
        const { controller, pages, selection, first, context } = await fixture(stage);
        const obsolete = controller.selectBranch('old');
        if (stage !== 'selection')
            await vi.waitFor(() =>
                expect(pages).toHaveBeenCalledWith(
                    expect.objectContaining({
                        branch_id: 'old',
                        limit: stage === 'first' ? 30 : 128,
                    }),
                ),
            );
        await controller.selectBranch('new');
        const current = get(controller.state);
        selection.resolve(selected('old'));
        first.resolve(page('old', 30));
        context.resolve(page('old', 128));
        await obsolete;
        expect(get(controller.state).messages).toBe(current.messages);
        expect(get(controller.state).conversation_state).toBe(current.conversation_state);
        expect(current).toMatchObject({
            conversation_state: selected('new'),
            messages: { phase: 'ready' },
        });
        expect(
            pages.mock.calls
                .filter(([input]) => input.branch_id === 'old')
                .map(([input]) => input.limit),
        ).toEqual(stage === 'selection' ? [] : stage === 'first' ? [30] : [30, 128]);
        controller.destroy();
    },
);
