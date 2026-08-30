import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
    ConversationBranchDto,
    ConversationDto,
    ConversationStateDto,
    MessageDto,
} from '../../lib/ipc/contracts';
import { LorepiaAppController } from '../app-controller';
import { createAppControllerFixture, deferred } from './app-controller-test-support';

const { character, conversation, greetingCatalog, conversationState, branch, mockClient } =
    createAppControllerFixture();

afterEach(() => {
    vi.useRealTimers();
});

describe('LorepiaAppController greeting-bound conversation entry', () => {
    it('loads the identity-only catalog with conversations and prefers the first enabled default', async () => {
        const controller = new LorepiaAppController(mockClient({}));

        await controller.selectCharacter(character);

        expect(get(controller.state).greeting_catalog).toEqual({
            phase: 'ready',
            error: null,
            value: greetingCatalog,
            selected_greeting_id: 'default-enabled',
        });
        expect(get(controller.state).conversations).toEqual({
            phase: 'ready',
            error: null,
            items: [conversation],
        });
    });

    it('creates a new room with the exact revision and selected greeting without reopening it', async () => {
        const createdConversation: ConversationDto = {
            ...conversation,
            id: 'conversation-created',
            title: character.name,
        };
        const createdState: ConversationStateDto = {
            ...conversationState,
            conversation_id: createdConversation.id,
        };
        const createdBranch: ConversationBranchDto = {
            ...branch,
            id: 'branch-created',
            conversation_id: createdConversation.id,
        };
        createdState.active_branch_id = createdBranch.id;
        const createConversation = vi.fn(() => Promise.resolve(createdConversation));
        const openExistingConversation = vi.fn(() => Promise.resolve(createdConversation));
        const client = mockClient({
            createConversation,
            openExistingConversation,
            getConversationState: () => Promise.resolve(createdState),
            listBranches: () => Promise.resolve([createdBranch]),
        });
        const controller = new LorepiaAppController(client);
        await controller.selectCharacter(character);

        expect(controller.selectGreeting('alternate-second')).toBe(true);
        expect(controller.selectGreeting('default-disabled')).toBe(false);
        await expect(controller.openNewConversation()).resolves.toBe(true);

        expect(createConversation).toHaveBeenCalledWith(character.id, character.name, 'chat', {
            character_content_revision_id: greetingCatalog.character_content_revision_id,
            greeting_id: 'alternate-second',
        });
        expect(openExistingConversation).not.toHaveBeenCalled();
        expect(get(controller.state)).toMatchObject({
            selected_conversation: createdConversation,
            conversation_state: createdState,
            branches: [createdBranch],
            messages: { phase: 'ready', error: null },
        });
    });

    it('records an existing-room open before loading its state, branches, and messages', async () => {
        const calls: string[] = [];
        const client = mockClient({
            openExistingConversation: () => {
                calls.push('open');
                return Promise.resolve(conversation);
            },
            getConversationState: () => {
                calls.push('state');
                return Promise.resolve(conversationState);
            },
            listBranches: () => {
                calls.push('branches');
                return Promise.resolve([branch]);
            },
            listBranchMessages: () => {
                calls.push('messages');
                return Promise.resolve([]);
            },
        });
        const controller = new LorepiaAppController(client);

        await expect(controller.selectConversation(conversation)).resolves.toBe(true);

        expect(calls).toEqual(['open', 'state', 'branches', 'messages']);
    });
});

describe('LorepiaAppController message removal scope', () => {
    const messageOne: MessageDto = {
        id: 'message-1',
        conversation_id: conversation.id,
        parent_id: null,
        role: 'user',
        content: '첫 메시지',
        status: 'complete',
        generation_id: null,
        created_at: '2026-08-02T00:00:00Z',
    };

    it('reports the committed mutation and applied message refresh separately', async () => {
        let messageReads = 0;
        const rewoundBranch: ConversationBranchDto = { ...branch, head_message_id: null };
        const controller = new LorepiaAppController(
            mockClient({
                listBranchMessages: () => {
                    messageReads += 1;
                    return Promise.resolve(messageReads === 1 ? [messageOne] : []);
                },
                removeMessageFromBranch: () => Promise.resolve(rewoundBranch),
            }),
        );
        await controller.selectConversation(conversation);

        await expect(controller.removeMessage(messageOne.id)).resolves.toEqual({
            mutationCommitted: true,
            messagesRefreshed: true,
            scopeKey: `${conversation.id}:${branch.id}`,
        });
        expect(get(controller.state)).toMatchObject({
            branches: [rewoundBranch],
            messages: { phase: 'ready', items: [] },
        });
        controller.destroy();
    });

    it('keeps a committed mutation receipt when the message readback fails', async () => {
        let messageReads = 0;
        const rewoundBranch: ConversationBranchDto = { ...branch, head_message_id: null };
        const controller = new LorepiaAppController(
            mockClient({
                listBranchMessages: () => {
                    messageReads += 1;
                    return messageReads === 1
                        ? Promise.resolve([messageOne])
                        : Promise.reject(new Error('message readback failed'));
                },
                removeMessageFromBranch: () => Promise.resolve(rewoundBranch),
            }),
        );
        await controller.selectConversation(conversation);

        await expect(controller.removeMessage(messageOne.id)).resolves.toEqual({
            mutationCommitted: true,
            messagesRefreshed: false,
            scopeKey: `${conversation.id}:${branch.id}`,
        });
        const state = get(controller.state);
        expect(state.branches).toEqual([rewoundBranch]);
        expect(state.messages.phase).toBe('error');
        expect(state.messages.error).not.toBeNull();
        expect(state.messages.items).toEqual([messageOne]);
        controller.destroy();
    });

    it('ignores a stale removal receipt after the active branch changes', async () => {
        const nextBranch: ConversationBranchDto = { ...branch, id: 'branch-2' };
        const nextState: ConversationStateDto = {
            ...conversationState,
            active_branch_id: nextBranch.id,
        };
        const nextMessage: MessageDto = {
            ...messageOne,
            id: 'message-branch-2',
            content: '새 분기 메시지',
        };
        const receipt = deferred<ConversationBranchDto>();
        const listBranchMessages = vi.fn((branchId: string) =>
            Promise.resolve(branchId === nextBranch.id ? [nextMessage] : [messageOne]),
        );
        const removeMessageFromBranch = vi.fn(() => receipt.promise);
        const controller = new LorepiaAppController(
            mockClient({
                listBranches: () => Promise.resolve([branch, nextBranch]),
                listBranchMessages,
                selectBranch: () => Promise.resolve(nextState),
                removeMessageFromBranch,
            }),
        );
        await controller.selectConversation(conversation);
        listBranchMessages.mockClear();

        const pendingRemoval = controller.removeMessage(messageOne.id);
        expect(removeMessageFromBranch).toHaveBeenCalledOnce();
        await controller.selectBranch(nextBranch.id);
        const currentAnnouncement = get(controller.state).announcement;
        receipt.resolve({ ...branch, head_message_id: null });
        await expect(pendingRemoval).resolves.toEqual({
            mutationCommitted: true,
            messagesRefreshed: false,
            scopeKey: `${conversation.id}:${branch.id}`,
        });

        expect(get(controller.state)).toMatchObject({
            conversation_state: { active_branch_id: nextBranch.id },
            messages: { phase: 'ready', items: [nextMessage] },
            announcement: currentAnnouncement,
        });
        expect(listBranchMessages).toHaveBeenCalledTimes(1);
        expect(listBranchMessages).toHaveBeenCalledWith(nextBranch.id);
        controller.destroy();
    });

    it('uses the conversation epoch to reject an ABA removal receipt after returning to the same room snapshot', async () => {
        const initialBranch: ConversationBranchDto = {
            ...branch,
            head_message_id: messageOne.id,
        };
        const initialState: ConversationStateDto = {
            ...conversationState,
            active_branch_id: initialBranch.id,
        };
        const nextConversation: ConversationDto = {
            ...conversation,
            id: 'conversation-2',
            title: '두 번째 대화',
        };
        const nextBranch: ConversationBranchDto = {
            ...branch,
            id: 'branch-conversation-2',
            conversation_id: nextConversation.id,
            head_message_id: 'message-conversation-2',
        };
        const nextState: ConversationStateDto = {
            ...conversationState,
            conversation_id: nextConversation.id,
            active_branch_id: nextBranch.id,
        };
        const nextMessage: MessageDto = {
            ...messageOne,
            id: 'message-conversation-2',
            conversation_id: nextConversation.id,
            content: '두 번째 대화 메시지',
        };
        const receipt = deferred<ConversationBranchDto>();
        const listBranchMessages = vi.fn((branchId: string) =>
            Promise.resolve(branchId === nextBranch.id ? [nextMessage] : [messageOne]),
        );
        const removeMessageFromBranch = vi.fn(() => receipt.promise);
        const controller = new LorepiaAppController(
            mockClient({
                openExistingConversation: (conversationId) =>
                    Promise.resolve(
                        conversationId === nextConversation.id ? nextConversation : conversation,
                    ),
                getConversationState: (conversationId) =>
                    Promise.resolve(
                        conversationId === nextConversation.id ? nextState : initialState,
                    ),
                listBranches: (conversationId) =>
                    Promise.resolve(
                        conversationId === nextConversation.id ? [nextBranch] : [initialBranch],
                    ),
                listBranchMessages,
                removeMessageFromBranch,
            }),
        );
        await controller.selectConversation(conversation);
        listBranchMessages.mockClear();

        const pendingRemoval = controller.removeMessage(messageOne.id);
        expect(removeMessageFromBranch).toHaveBeenCalledWith({
            conversation_id: conversation.id,
            branch_id: initialBranch.id,
            expected_head: messageOne.id,
            message_id: messageOne.id,
        });
        await controller.selectConversation(nextConversation);
        await controller.selectConversation(conversation);
        const currentAnnouncement = get(controller.state).announcement;
        receipt.resolve({ ...initialBranch, head_message_id: null });
        await pendingRemoval;

        expect(get(controller.state)).toMatchObject({
            selected_conversation: conversation,
            conversation_state: initialState,
            branches: [initialBranch],
            messages: { phase: 'ready', items: [messageOne] },
            announcement: currentAnnouncement,
        });
        expect(listBranchMessages).toHaveBeenCalledTimes(2);
        expect(listBranchMessages).toHaveBeenNthCalledWith(1, nextBranch.id);
        expect(listBranchMessages).toHaveBeenNthCalledWith(2, initialBranch.id);
        controller.destroy();
    });

    it('ignores a stale message refresh after another conversation finishes loading', async () => {
        const nextConversation: ConversationDto = {
            ...conversation,
            id: 'conversation-2',
            title: '두 번째 대화',
        };
        const nextBranch: ConversationBranchDto = {
            ...branch,
            id: 'branch-conversation-2',
            conversation_id: nextConversation.id,
        };
        const nextState: ConversationStateDto = {
            ...conversationState,
            conversation_id: nextConversation.id,
            active_branch_id: nextBranch.id,
        };
        const nextMessage: MessageDto = {
            ...messageOne,
            id: 'message-conversation-2',
            conversation_id: nextConversation.id,
            content: '두 번째 대화 메시지',
        };
        const staleMessages = deferred<MessageDto[]>();
        let firstBranchLoads = 0;
        const listBranchMessages = vi.fn((branchId: string) => {
            if (branchId === nextBranch.id) return Promise.resolve([nextMessage]);
            firstBranchLoads += 1;
            return firstBranchLoads === 1 ? Promise.resolve([messageOne]) : staleMessages.promise;
        });
        const controller = new LorepiaAppController(
            mockClient({
                openExistingConversation: (conversationId) =>
                    Promise.resolve(
                        conversationId === nextConversation.id ? nextConversation : conversation,
                    ),
                getConversationState: (conversationId) =>
                    Promise.resolve(
                        conversationId === nextConversation.id ? nextState : conversationState,
                    ),
                listBranches: (conversationId) =>
                    Promise.resolve(
                        conversationId === nextConversation.id ? [nextBranch] : [branch],
                    ),
                listBranchMessages,
                removeMessageFromBranch: () =>
                    Promise.resolve({ ...branch, head_message_id: null }),
            }),
        );
        await controller.selectConversation(conversation);

        const pendingRemoval = controller.removeMessage(messageOne.id);
        await vi.waitFor(() => expect(firstBranchLoads).toBe(2));
        await controller.selectConversation(nextConversation);
        const currentAnnouncement = get(controller.state).announcement;
        staleMessages.resolve([]);
        await pendingRemoval;

        expect(get(controller.state)).toMatchObject({
            selected_conversation: nextConversation,
            conversation_state: nextState,
            messages: { phase: 'ready', items: [nextMessage] },
            announcement: currentAnnouncement,
        });
        controller.destroy();
    });

    it('suppresses an error from a removal that belongs to a stale branch', async () => {
        const nextBranch: ConversationBranchDto = { ...branch, id: 'branch-2' };
        const nextState: ConversationStateDto = {
            ...conversationState,
            active_branch_id: nextBranch.id,
        };
        const receipt = deferred<ConversationBranchDto>();
        const controller = new LorepiaAppController(
            mockClient({
                listBranches: () => Promise.resolve([branch, nextBranch]),
                selectBranch: () => Promise.resolve(nextState),
                removeMessageFromBranch: () => receipt.promise,
            }),
        );
        await controller.selectConversation(conversation);

        const pendingRemoval = controller.removeMessage(messageOne.id);
        await controller.selectBranch(nextBranch.id);
        const currentAnnouncement = get(controller.state).announcement;
        receipt.reject(new Error('stale removal failed'));
        await pendingRemoval;

        expect(get(controller.state).announcement).toBe(currentAnnouncement);
        controller.destroy();
    });

    it('rejects a removal receipt whose route does not match the requested branch', async () => {
        const wrongBranch: ConversationBranchDto = { ...branch, id: 'branch-wrong' };
        const listBranchMessages = vi.fn(() => Promise.resolve([messageOne]));
        const controller = new LorepiaAppController(
            mockClient({
                listBranchMessages,
                removeMessageFromBranch: () => Promise.resolve(wrongBranch),
            }),
        );
        await controller.selectConversation(conversation);
        listBranchMessages.mockClear();

        await expect(controller.removeMessage(messageOne.id)).resolves.toEqual({
            mutationCommitted: false,
            messagesRefreshed: false,
            scopeKey: `${conversation.id}:${branch.id}`,
        });

        expect(listBranchMessages).not.toHaveBeenCalled();
        expect(get(controller.state).messages.items).toEqual([messageOne]);
        expect(get(controller.state).announcement).not.toBe('이 메시지부터 분기에서 제거했습니다.');
        controller.destroy();
    });

    it('rejects a removal receipt with the requested branch id but a different conversation id', async () => {
        const wrongConversationBranch: ConversationBranchDto = {
            ...branch,
            conversation_id: 'conversation-wrong',
        };
        const listBranchMessages = vi.fn(() => Promise.resolve([messageOne]));
        const controller = new LorepiaAppController(
            mockClient({
                listBranchMessages,
                removeMessageFromBranch: () => Promise.resolve(wrongConversationBranch),
            }),
        );
        await controller.selectConversation(conversation);
        listBranchMessages.mockClear();

        await controller.removeMessage(messageOne.id);

        expect(listBranchMessages).not.toHaveBeenCalled();
        expect(get(controller.state).messages.items).toEqual([messageOne]);
        expect(get(controller.state).announcement).toBe(
            '메시지 제거 결과가 요청한 대화 분기와 일치하지 않습니다.',
        );
        controller.destroy();
    });

    it('does not let an older same-room result overwrite a newer branch head', async () => {
        const initialBranch: ConversationBranchDto = {
            ...branch,
            head_message_id: 'message-head',
        };
        const newerBranch: ConversationBranchDto = {
            ...initialBranch,
            head_message_id: 'message-newer-head',
        };
        const staleBranch: ConversationBranchDto = {
            ...initialBranch,
            head_message_id: 'message-stale-head',
        };
        const newerMessage: MessageDto = {
            ...messageOne,
            id: 'message-newer-head',
            content: '최신 제거 결과',
        };
        const staleMessage: MessageDto = {
            ...messageOne,
            id: 'message-stale-head',
            content: '뒤늦은 제거 결과',
        };
        const staleReceipt = deferred<ConversationBranchDto>();
        let removalCalls = 0;
        let messageLoads = 0;
        const listBranchMessages = vi.fn(() => {
            messageLoads += 1;
            if (messageLoads === 1) return Promise.resolve([messageOne]);
            if (messageLoads === 2) return Promise.resolve([newerMessage]);
            return Promise.resolve([staleMessage]);
        });
        const controller = new LorepiaAppController(
            mockClient({
                listBranches: () => Promise.resolve([initialBranch]),
                listBranchMessages,
                removeMessageFromBranch: () => {
                    removalCalls += 1;
                    return removalCalls === 1 ? staleReceipt.promise : Promise.resolve(newerBranch);
                },
            }),
        );
        await controller.selectConversation(conversation);

        const olderRemoval = controller.removeMessage('message-old');
        await controller.removeMessage('message-newer');
        staleReceipt.resolve(staleBranch);
        await olderRemoval;

        expect(get(controller.state).branches).toContainEqual(newerBranch);
        expect(get(controller.state).messages.items).toEqual([newerMessage]);
        expect(listBranchMessages).toHaveBeenCalledTimes(2);
        controller.destroy();
    });

    it('does not let an older same-room refresh overwrite a newer completed refresh', async () => {
        const initialBranch: ConversationBranchDto = {
            ...branch,
            head_message_id: 'message-initial-head',
        };
        const olderBranch: ConversationBranchDto = {
            ...initialBranch,
            head_message_id: 'message-older-head',
        };
        const newerBranch: ConversationBranchDto = {
            ...initialBranch,
            head_message_id: 'message-newer-head',
        };
        const initialMessage: MessageDto = {
            ...messageOne,
            id: 'message-initial-head',
        };
        const olderMessage: MessageDto = {
            ...messageOne,
            id: 'message-older-head',
            content: '뒤늦은 새로고침 결과',
        };
        const newerMessage: MessageDto = {
            ...messageOne,
            id: 'message-newer-head',
            content: '최신 새로고침 결과',
        };
        const olderRefresh = deferred<MessageDto[]>();
        const newerRefresh = deferred<MessageDto[]>();
        let removalCalls = 0;
        let messageLoads = 0;
        const listBranchMessages = vi.fn(() => {
            messageLoads += 1;
            if (messageLoads === 1) return Promise.resolve([initialMessage]);
            if (messageLoads === 2) return olderRefresh.promise;
            return newerRefresh.promise;
        });
        const controller = new LorepiaAppController(
            mockClient({
                listBranches: () => Promise.resolve([initialBranch]),
                listBranchMessages,
                removeMessageFromBranch: () => {
                    removalCalls += 1;
                    return Promise.resolve(removalCalls === 1 ? olderBranch : newerBranch);
                },
            }),
        );
        await controller.selectConversation(conversation);

        const olderRemoval = controller.removeMessage('message-old');
        const newerRemoval = controller.removeMessage('message-newer');
        await vi.waitFor(() => expect(listBranchMessages).toHaveBeenCalledTimes(3));

        newerRefresh.resolve([newerMessage]);
        await newerRemoval;
        expect(get(controller.state)).toMatchObject({
            branches: [newerBranch],
            messages: { phase: 'ready', items: [newerMessage] },
        });

        olderRefresh.resolve([olderMessage]);
        await olderRemoval;

        expect(get(controller.state)).toMatchObject({
            branches: [newerBranch],
            messages: { phase: 'ready', items: [newerMessage] },
        });
        controller.destroy();
    });
});
