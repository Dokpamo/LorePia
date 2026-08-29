import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
    CharacterDto,
    CharacterGreetingCatalogDto,
    ChatEventDto,
    ChatStreamItemDto,
    ConversationBranchDto,
    ConversationDto,
    ConversationStateDto,
    EditUserMessageInput,
    GenerationSelectionInput,
    LorepiaClient,
    MemorySupervisorStatusDto,
    MessageDto,
    RegenerateAssistantMessageInput,
    SendMessageInput,
} from '../lib/ipc/contracts';
import { LorepiaClientError } from '../lib/ipc/errors';
import { LorepiaAppController } from './app-controller';

const character: CharacterDto = {
    id: 'character-1',
    name: '라온',
    description: '',
    source_hash: 'synthetic',
    avatar_asset_id: null,
    created_at: '2026-08-02T00:00:00Z',
};

const conversation: ConversationDto = {
    id: 'conversation-1',
    character_id: character.id,
    title: '첫 대화',
    created_at: '2026-08-02T00:00:00Z',
    updated_at: '2026-08-02T00:00:00Z',
};

const greetingCatalog: CharacterGreetingCatalogDto = {
    character_id: character.id,
    character_content_revision_id: 'character-revision-7',
    greetings: [
        { id: 'alternate-first', kind: 'alternate', enabled: true },
        { id: 'default-disabled', kind: 'default', enabled: false },
        { id: 'default-enabled', kind: 'default', enabled: true },
        { id: 'alternate-second', kind: 'alternate', enabled: true },
    ],
};

const conversationState: ConversationStateDto = {
    conversation_id: conversation.id,
    active_branch_id: 'branch-1',
    selected_mode: 'chat',
    updated_at: '2026-08-02T00:00:00Z',
};

const branch: ConversationBranchDto = {
    id: conversationState.active_branch_id,
    conversation_id: conversation.id,
    title: null,
    fork_message_id: null,
    head_message_id: null,
    created_at: '2026-08-02T00:00:00Z',
    updated_at: '2026-08-02T00:00:00Z',
};

function mockClient(overrides: Partial<LorepiaClient>): LorepiaClient {
    const defaults: Partial<LorepiaClient> = {
        bootstrapSnapshot: () =>
            Promise.resolve({
                shell_api_version: 3,
                core_api_version: 10,
                chat_event_version: 4,
                health: {
                    core_version: '0.1.0',
                    database_open: true,
                    schema_version: 1,
                    data_root_writable: true,
                    staging_writable: true,
                    recovery_pending: false,
                    active_jobs: 0,
                },
            }),
        getMemorySupervisorStatus: () =>
            Promise.resolve({
                sequence: 1,
                phase: 'running',
                recovered_interrupted_jobs: 0,
                completed_jobs: 0,
            }),
        subscribeMemorySupervisorStatus: () => Promise.resolve(() => undefined),
        listCharacters: () => Promise.resolve([character]),
        getProviderOverview: () =>
            Promise.resolve({
                templates: [],
                connections: [],
                legacy_profiles: [],
                settings: {
                    preserve_partial_generations: true,
                    selected_provider_profile_id: null,
                    selected_model_route_id: 'route-1',
                    selected_generation_preset_id: 'preset-1',
                },
            }),
        listProviderDiscoveries: () => Promise.resolve([]),
        providerCatalogStatus: () =>
            Promise.resolve({
                status_schema_version: 1,
                state_version: 1,
                active_revision: 1,
                active_snapshot_sha256: 'synthetic-active',
                bundled_baseline_sha256: 'synthetic-baseline',
                snapshot_count: 1,
                signed_update_count: 0,
                highest_accepted_revision: 1,
                latest_issued_at: null,
                active_signed_revisions: [],
            }),
        providerCatalogHistory: () =>
            Promise.resolve({
                history_schema_version: 1,
                active_revision: 1,
                revisions: [],
                activations: [],
                next_before_revision: null,
                next_before_state_version: null,
            }),
        listConversations: () => Promise.resolve([conversation]),
        getCharacterGreetingCatalog: () => Promise.resolve(greetingCatalog),
        openExistingConversation: () => Promise.resolve(conversation),
        getConversationState: () => Promise.resolve(conversationState),
        listBranches: () => Promise.resolve([branch]),
        listBranchMessages: () => Promise.resolve([]),
        listRetryableMemoryQueryEmbeddings: () => Promise.resolve([]),
        listInterruptedMemoryJobs: () => Promise.resolve([]),
        disposeChatStream: () => Promise.resolve(false),
    };
    return new Proxy({ ...defaults, ...overrides } as LorepiaClient, {
        get(target, property, receiver) {
            const value = Reflect.get(target, property, receiver) as unknown;
            if (typeof value === 'function') return value;
            throw new Error(`Unexpected client method: ${String(property)}`);
        },
    });
}

function deferred<T>(): {
    promise: Promise<T>;
    resolve: (value: T) => void;
    reject: (reason: unknown) => void;
} {
    let resolve!: (value: T) => void;
    let reject!: (reason: unknown) => void;
    const promise = new Promise<T>((nextResolve, nextReject) => {
        resolve = nextResolve;
        reject = nextReject;
    });
    return { promise, resolve, reject };
}

afterEach(() => {
    vi.useRealTimers();
});

function textEvent(sequence: number, payload = '늦게 도착한 조각'): ChatStreamItemDto {
    const event: ChatEventDto = {
        event_version: 4,
        generation_id: 'generation-1',
        conversation_id: conversation.id,
        branch_id: branch.id,
        assistant_message_id: 'message-1',
        sequence,
        emitted_at: '2026-08-02T00:00:00Z',
        kind: { type: 'text_delta', payload },
    };
    return { type: 'event', payload: event };
}

function reasoningEvent(sequence: number, payload = '마지막 추론 조각'): ChatStreamItemDto {
    return {
        type: 'event',
        payload: {
            event_version: 4,
            generation_id: 'generation-1',
            conversation_id: conversation.id,
            branch_id: branch.id,
            assistant_message_id: 'message-1',
            sequence,
            emitted_at: '2026-08-02T00:00:00Z',
            kind: { type: 'reasoning_delta', payload },
        },
    };
}

function terminalEvent(sequence: number): ChatStreamItemDto {
    return {
        type: 'event',
        payload: {
            event_version: 4,
            generation_id: 'generation-1',
            conversation_id: conversation.id,
            branch_id: branch.id,
            assistant_message_id: 'message-1',
            sequence,
            emitted_at: '2026-08-02T00:00:00Z',
            kind: { type: 'generation_finished' },
        },
    };
}

describe('LorepiaAppController ABI compatibility', () => {
    it.each([
        ['shell', 1, 9],
        ['Core', 2, 8],
    ])('rejects a stale %s API before loading product data', async (_label, shell, core) => {
        const listCharacters = vi.fn().mockResolvedValue([character]);
        const client = mockClient({
            bootstrapSnapshot: () =>
                Promise.resolve({
                    shell_api_version: shell,
                    core_api_version: core,
                    chat_event_version: 4,
                    health: {
                        core_version: '0.1.0',
                        database_open: true,
                        schema_version: 1,
                        data_root_writable: true,
                        staging_writable: true,
                        recovery_pending: false,
                        active_jobs: 0,
                    },
                }),
            listCharacters,
        });
        const controller = new LorepiaAppController(client);

        await controller.start();

        expect(get(controller.state).bootstrap).toMatchObject({
            phase: 'error',
            error: '앱과 Core 버전이 호환되지 않습니다.',
        });
        expect(listCharacters).not.toHaveBeenCalled();
    });
});

describe('LorepiaAppController memory supervisor status', () => {
    it('subscribes before the snapshot, ignores stale events and detaches on destroy', async () => {
        let emitStatus: (status: MemorySupervisorStatusDto) => void = () => {
            throw new Error('memory supervisor listener was not connected');
        };
        const unlisten = vi.fn();
        const client = mockClient({
            subscribeMemorySupervisorStatus: (onStatus) => {
                emitStatus = onStatus;
                return Promise.resolve(unlisten);
            },
            getMemorySupervisorStatus: () =>
                Promise.resolve({
                    sequence: 2,
                    phase: 'running',
                    recovered_interrupted_jobs: 1,
                    completed_jobs: 3,
                }),
        });
        const controller = new LorepiaAppController(client);

        await controller.start();
        expect(get(controller.state).memory_supervisor).toEqual({
            phase: 'ready',
            error: null,
            status: {
                sequence: 2,
                phase: 'running',
                recovered_interrupted_jobs: 1,
                completed_jobs: 3,
            },
        });

        emitStatus({
            sequence: 1,
            phase: 'failed',
            recovered_interrupted_jobs: 999,
            completed_jobs: 999,
        });
        expect(get(controller.state).memory_supervisor.status?.sequence).toBe(2);

        emitStatus({
            sequence: 3,
            phase: 'recovered',
            recovered_interrupted_jobs: 2,
            completed_jobs: 4,
        });
        expect(get(controller.state).memory_supervisor.status).toMatchObject({
            sequence: 3,
            phase: 'recovered',
            recovered_interrupted_jobs: 2,
            completed_jobs: 4,
        });

        controller.destroy();
        expect(unlisten).toHaveBeenCalledOnce();
    });

    it('keeps the snapshot visible while reporting a failed live subscription', async () => {
        const controller = new LorepiaAppController(
            mockClient({
                subscribeMemorySupervisorStatus: () =>
                    Promise.reject(new Error('event permission denied')),
                getMemorySupervisorStatus: () =>
                    Promise.resolve({
                        sequence: 4,
                        phase: 'running',
                        recovered_interrupted_jobs: 1,
                        completed_jobs: 8,
                    }),
            }),
        );

        await controller.start();

        expect(get(controller.state).memory_supervisor).toEqual({
            phase: 'ready',
            error: '기억 작업 상태의 실시간 갱신을 연결하지 못했습니다.',
            status: {
                sequence: 4,
                phase: 'running',
                recovered_interrupted_jobs: 1,
                completed_jobs: 8,
            },
        });
        controller.destroy();
    });
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

describe('LorepiaAppController memory query retry', () => {
    it.each(['edit', 'regenerate'] as const)(
        'refreshes retry candidates for the new branch after a successful %s',
        async (action) => {
            const nextBranch: ConversationBranchDto = {
                ...branch,
                id: 'branch-2',
                fork_message_id: 'message-user',
            };
            const nextConversationState: ConversationStateDto = {
                ...conversationState,
                active_branch_id: nextBranch.id,
            };
            const staleCandidate = {
                id: 'query-embedding-old-branch',
                status: 'failed' as const,
                revision: 3,
                conversation_id: conversation.id,
                branch_id: branch.id,
                error_code: 'provider_unavailable',
                requires_unknown_outcome_acknowledgement: false,
            };
            const nextCandidate = {
                ...staleCandidate,
                id: 'query-embedding-new-branch',
                revision: 1,
                branch_id: nextBranch.id,
            };
            const list = vi.fn(({ branch_id }: { branch_id: string }) =>
                Promise.resolve(branch_id === nextBranch.id ? [nextCandidate] : [staleCandidate]),
            );
            const startBranchGeneration = vi.fn(() =>
                Promise.resolve({ branch: nextBranch, generation_id: 'generation-2' }),
            );
            const client = mockClient({
                listRetryableMemoryQueryEmbeddings: list,
                listBranchMessages: () => Promise.resolve([]),
                selectBranch: () => Promise.resolve(nextConversationState),
                editUserMessage: startBranchGeneration,
                regenerateAssistantMessage: startBranchGeneration,
            });
            const controller = new LorepiaAppController(client);
            await controller.start();
            await controller.selectCharacter(character);
            await controller.selectConversation(conversation);
            await vi.waitFor(() =>
                expect(get(controller.state).memory_query_retries.candidates).toEqual([
                    staleCandidate,
                ]),
            );

            const succeeded =
                action === 'edit'
                    ? await controller.editUserMessage('message-user', '고친 메시지')
                    : await controller.regenerateAssistantMessage('message-assistant');

            expect(succeeded).toBe(true);
            expect(get(controller.state).conversation_state?.active_branch_id).toBe(nextBranch.id);
            await vi.waitFor(() =>
                expect(get(controller.state).memory_query_retries.candidates).toEqual([
                    nextCandidate,
                ]),
            );
            expect(get(controller.state).memory_query_retries.candidates).not.toContainEqual(
                staleCandidate,
            );
            expect(list).toHaveBeenCalledWith({
                conversation_id: conversation.id,
                branch_id: nextBranch.id,
                limit: 16,
            });
            controller.destroy();
        },
    );

    it('requires positive unknown-outcome acknowledgement and preserves the exact CAS revision', async () => {
        const candidate = {
            id: 'query-embedding-1',
            status: 'interrupted' as const,
            revision: 4,
            conversation_id: conversation.id,
            branch_id: branch.id,
            error_code: 'provider_unavailable',
            requires_unknown_outcome_acknowledgement: true,
        };
        const retry = vi.fn().mockResolvedValue({
            id: 'query-embedding-1',
            status: 'queued',
            revision: 5,
            conversation_id: conversation.id,
            branch_id: branch.id,
            error_code: null,
            requires_unknown_outcome_acknowledgement: false,
        });
        const list = vi.fn().mockResolvedValue([candidate]);
        const client = mockClient({
            listRetryableMemoryQueryEmbeddings: list,
            retryMemoryQueryEmbedding: retry,
        });
        const controller = new LorepiaAppController(client);
        await controller.selectConversation(conversation);
        await vi.waitFor(() =>
            expect(get(controller.state).memory_query_retries.candidates).toEqual([candidate]),
        );
        expect(list).toHaveBeenCalledWith({
            conversation_id: conversation.id,
            branch_id: branch.id,
            limit: 16,
        });

        await expect(controller.retryMemoryQueryEmbedding(candidate, false)).resolves.toBe(false);
        expect(retry).not.toHaveBeenCalled();

        await expect(controller.retryMemoryQueryEmbedding(candidate, true)).resolves.toBe(true);
        expect(retry).toHaveBeenCalledWith({
            conversation_id: candidate.conversation_id,
            branch_id: candidate.branch_id,
            id: candidate.id,
            expected_revision: 4,
            acknowledge_unknown_outcome: true,
        });
        expect(get(controller.state).memory_query_retries).toMatchObject({
            phase: 'ready',
            error: null,
            candidates: [],
            busy_id: null,
        });
        expect(get(controller.state).memory_query_retries.notice).toContain(
            '미리보기나 메시지 결과는 만들지 않았습니다',
        );
        expect(get(controller.state).memory_query_retries.notice).toContain(
            '계획 미리보기 또는 메시지 전송·편집·재생성',
        );
    });

    it.each(['failed', 'cancelled'] as const)(
        'retries a %s preparation without unknown-outcome acknowledgement',
        async (status) => {
            const candidate = {
                id: `query-embedding-${status}`,
                status,
                revision: 8,
                conversation_id: conversation.id,
                branch_id: branch.id,
                error_code: status === 'failed' ? 'provider_unavailable' : null,
                requires_unknown_outcome_acknowledgement: false,
            };
            const retry = vi.fn().mockResolvedValue({
                ...candidate,
                status: 'queued',
                revision: 9,
                error_code: null,
            });
            const controller = new LorepiaAppController(
                mockClient({
                    listRetryableMemoryQueryEmbeddings: () => Promise.resolve([candidate]),
                    retryMemoryQueryEmbedding: retry,
                }),
            );
            await controller.selectConversation(conversation);
            await vi.waitFor(() =>
                expect(get(controller.state).memory_query_retries.candidates).toEqual([candidate]),
            );

            await expect(controller.retryMemoryQueryEmbedding(candidate, false)).resolves.toBe(
                true,
            );

            expect(retry).toHaveBeenCalledWith({
                conversation_id: candidate.conversation_id,
                branch_id: candidate.branch_id,
                id: candidate.id,
                expected_revision: 8,
                acknowledge_unknown_outcome: false,
            });
        },
    );

    it('pins the CAS revision and requires acknowledgement for an interrupted memory job', async () => {
        const job = {
            memory_job_id: 'memory-job-1',
            kind: 'summary' as const,
            revision: 5,
            conversation_id: conversation.id,
            branch_id: branch.id,
            source_start_message_id: 'message-1',
            source_end_message_id: 'message-2',
            attempt: 1,
            interruption_count: 1,
            last_interrupted_at: '2026-01-01T00:00:00Z',
            last_error_code: 'process_restarted',
        };
        const retry = vi.fn().mockResolvedValue({
            memory_job_id: job.memory_job_id,
            kind: 'summary',
            status: 'queued',
            revision: 6,
            conversation_id: job.conversation_id,
            branch_id: job.branch_id,
            source_start_message_id: job.source_start_message_id,
            source_end_message_id: job.source_end_message_id,
            attempt: 1,
        });
        const controller = new LorepiaAppController(
            mockClient({
                listInterruptedMemoryJobs: () => Promise.resolve([job]),
                retryInterruptedMemoryJob: retry,
            }),
        );
        await controller.selectConversation(conversation);
        await vi.waitFor(() =>
            expect(get(controller.state).memory_query_retries.interrupted_jobs).toEqual([job]),
        );

        await expect(controller.retryInterruptedMemoryJob(job, false)).resolves.toBe(false);
        expect(retry).not.toHaveBeenCalled();

        await expect(controller.retryInterruptedMemoryJob(job, true)).resolves.toBe(true);
        expect(retry).toHaveBeenCalledWith({
            conversation_id: job.conversation_id,
            branch_id: job.branch_id,
            memory_job_id: job.memory_job_id,
            expected_revision: 5,
            acknowledge_unknown_outcome: true,
        });
        expect(get(controller.state).memory_query_retries.interrupted_jobs).toEqual([]);
    });

    it('keeps embedding retry candidates usable when the interrupted job listing fails', async () => {
        const candidate = {
            id: 'query-embedding-1',
            status: 'failed' as const,
            revision: 2,
            conversation_id: conversation.id,
            branch_id: branch.id,
            error_code: 'provider_unavailable',
            requires_unknown_outcome_acknowledgement: false,
        };
        const controller = new LorepiaAppController(
            mockClient({
                listRetryableMemoryQueryEmbeddings: () => Promise.resolve([candidate]),
                listInterruptedMemoryJobs: () => Promise.reject(new Error('listing unavailable')),
            }),
        );
        await controller.selectConversation(conversation);
        await vi.waitFor(() =>
            expect(get(controller.state).memory_query_retries.candidates).toEqual([candidate]),
        );

        expect(get(controller.state).memory_query_retries.interrupted_jobs).toEqual([]);
        expect(get(controller.state).memory_query_retries.phase).toBe('ready');
    });

    it('retains the candidate and reports an error when the retry receipt cannot be verified', async () => {
        const candidate = {
            id: 'query-embedding-failed',
            status: 'failed' as const,
            revision: 12,
            conversation_id: conversation.id,
            branch_id: branch.id,
            error_code: 'provider_unavailable',
            requires_unknown_outcome_acknowledgement: false,
        };
        const controller = new LorepiaAppController(
            mockClient({
                listRetryableMemoryQueryEmbeddings: () => Promise.resolve([candidate]),
                retryMemoryQueryEmbedding: () =>
                    Promise.resolve({
                        ...candidate,
                        status: 'queued',
                        revision: 99,
                        error_code: null,
                    }),
            }),
        );
        await controller.selectConversation(conversation);
        await vi.waitFor(() =>
            expect(get(controller.state).memory_query_retries.candidates).toEqual([candidate]),
        );

        await expect(controller.retryMemoryQueryEmbedding(candidate, false)).resolves.toBe(false);

        expect(get(controller.state).memory_query_retries).toMatchObject({
            phase: 'error',
            error: '재시도 결과를 검증하지 못했습니다. 목록을 새로고침해 상태를 확인하세요.',
            candidates: [candidate],
            busy_id: null,
            notice: null,
        });
        expect(get(controller.state).announcement).not.toContain(
            '임베딩 준비만 다시 대기열에 넣었습니다',
        );
    });
});

describe('LorepiaAppController ordinary generation operation nonce', () => {
    const permissionDenied = () =>
        Object.assign(new Error('permission denied'), {
            code: 'permission_denied',
            message_key: 'error.permission_denied',
            recoverable: true,
            operation_id: null,
            field_errors: [],
        });

    async function readyController(
        overrides: Partial<LorepiaClient>,
    ): Promise<LorepiaAppController> {
        const controller = new LorepiaAppController(mockClient(overrides));
        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        return controller;
    }

    function nonceOf(input: { operation_nonce?: string | null }): string {
        const nonce = input.operation_nonce;
        if (typeof nonce !== 'string') throw new Error('operation nonce is missing');
        expect(nonce).toEqual(expect.any(String));
        return nonce;
    }

    function itemAt<T>(items: readonly T[], index: number): T {
        const item = items[index];
        if (item === undefined) throw new Error(`missing item at index ${String(index)}`);
        return item;
    }

    it.each(['', 'attempt\u0000id', '가'.repeat(171), 'a'.repeat(257)])(
        'rejects a malformed staged attempt identifier: %j',
        (generationAttemptId) => {
            const controller = new LorepiaAppController(mockClient({}));
            expect(controller.stageGenerationAttemptRetry(generationAttemptId)).toBe(false);
            controller.destroy();
        },
    );

    it('reuses only the exact denied send and rotates for explicit abandon or caller-owned drift', async () => {
        const inputs: SendMessageInput[] = [];
        const streamIds: string[] = [];
        const targetA = {
            model_route_id: 'route-room-a',
            generation_preset_id: 'preset-room-a',
        };
        const targetB = {
            model_route_id: 'route-room-b',
            generation_preset_id: 'preset-room-b',
        };
        const sendMessage = vi.fn((input: SendMessageInput, streamId: string) => {
            inputs.push(structuredClone(input));
            streamIds.push(streamId);
            return Promise.reject(permissionDenied());
        });
        const controller = await readyController({
            sendMessage,
            setConversationMode: (_conversationId, mode) =>
                Promise.resolve({ ...conversationState, selected_mode: mode }),
            removeMessageFromBranch: () =>
                Promise.resolve({ ...branch, head_message_id: 'message-new-head' }),
        });
        controller.setRoomGenerationTarget(conversation.id, branch.id, targetA);

        await expect(controller.sendMessage('  같은 요청  ')).resolves.toBe(false);
        await expect(controller.sendMessage('같은 요청')).resolves.toBe(false);
        expect(inputs[0]).toEqual({
            conversation_id: conversation.id,
            branch_id: branch.id,
            expected_head: null,
            mode: 'chat',
            text: '같은 요청',
            selection: { kind: 'target', target: targetA },
            operation_nonce: nonceOf(itemAt(inputs, 0)),
        });
        expect(inputs[1]).toEqual(inputs[0]);

        controller.beginNewGenerationOperation();
        await expect(controller.sendMessage('같은 요청')).resolves.toBe(false);
        expect(nonceOf(itemAt(inputs, 2))).not.toBe(nonceOf(itemAt(inputs, 1)));

        await expect(controller.sendMessage('달라진 요청')).resolves.toBe(false);
        expect(nonceOf(itemAt(inputs, 3))).not.toBe(nonceOf(itemAt(inputs, 2)));

        controller.setRoomGenerationTarget(conversation.id, branch.id, targetB);
        await expect(controller.sendMessage('같은 요청')).resolves.toBe(false);
        expect(inputs[4]?.selection).toEqual({ kind: 'target', target: targetB });
        expect(nonceOf(itemAt(inputs, 4))).not.toBe(nonceOf(itemAt(inputs, 3)));

        await controller.setConversationMode('story');
        await expect(controller.sendMessage('같은 요청')).resolves.toBe(false);
        expect(inputs[5]?.mode).toBe('story');
        expect(nonceOf(itemAt(inputs, 5))).toBe(nonceOf(itemAt(inputs, 4)));

        await controller.removeMessage('message-old-head');
        await expect(controller.sendMessage('같은 요청')).resolves.toBe(false);
        expect(inputs[6]?.expected_head).toBe('message-new-head');
        expect(nonceOf(itemAt(inputs, 6))).not.toBe(nonceOf(itemAt(inputs, 5)));
        for (const [index, input] of inputs.entries()) {
            expect(streamIds[index]).not.toBe(nonceOf(input));
        }
        controller.destroy();
    });

    it('sends character runtime variables and treats a changed value as request drift', async () => {
        const inputs: SendMessageInput[] = [];
        const controller = await readyController({
            sendMessage: (input) => {
                inputs.push(structuredClone(input));
                return Promise.reject(permissionDenied());
            },
        });
        const enabled = {
            values: [
                {
                    variable: {
                        scope: 'character' as const,
                        namespace: null,
                        id: 'background_music',
                    },
                    value: { type: 'text' as const, value: '1' },
                },
            ],
        };
        const disabled = {
            values: [
                {
                    variable: {
                        scope: 'character' as const,
                        namespace: null,
                        id: 'background_music',
                    },
                    value: { type: 'text' as const, value: '0' },
                },
            ],
        };

        await expect(controller.sendMessage('같은 요청', enabled)).resolves.toBe(false);
        await expect(controller.sendMessage('같은 요청', enabled)).resolves.toBe(false);
        await expect(controller.sendMessage('같은 요청', disabled)).resolves.toBe(false);

        expect(inputs[0]?.variable_overrides).toEqual(enabled);
        expect(inputs[1]?.variable_overrides).toEqual(enabled);
        expect(nonceOf(itemAt(inputs, 1))).toBe(nonceOf(itemAt(inputs, 0)));
        expect(inputs[2]?.variable_overrides).toEqual(disabled);
        expect(nonceOf(itemAt(inputs, 2))).not.toBe(nonceOf(itemAt(inputs, 1)));
        controller.destroy();
    });

    it('uses an approved attempt id as the exclusive retry authority and rejects identity drift', async () => {
        const inputs: SendMessageInput[] = [];
        const controller = await readyController({
            sendMessage: (input) => {
                inputs.push(structuredClone(input));
                return Promise.reject(permissionDenied());
            },
            setConversationMode: (_conversationId, mode) =>
                Promise.resolve({ ...conversationState, selected_mode: mode }),
        });

        await expect(controller.sendMessage('승인할 요청')).resolves.toBe(false);
        const originalNonce = nonceOf(itemAt(inputs, 0));
        expect(controller.stageGenerationAttemptRetry('generation-attempt-approved')).toBe(true);
        await controller.setConversationMode('story');
        await expect(controller.sendMessage('승인할 요청')).resolves.toBe(false);
        await expect(controller.sendMessage('승인할 요청')).resolves.toBe(false);

        for (const resumed of [itemAt(inputs, 1), itemAt(inputs, 2)]) {
            expect(resumed).toMatchObject({
                mode: 'story',
                text: '승인할 요청',
                generation_attempt_id: 'generation-attempt-approved',
            });
            expect(resumed).not.toHaveProperty('operation_nonce');
        }

        await expect(controller.sendMessage('바뀐 요청')).resolves.toBe(false);
        expect(itemAt(inputs, 3)).not.toHaveProperty('generation_attempt_id');
        expect(nonceOf(itemAt(inputs, 3))).not.toBe(originalNonce);

        expect(controller.stageGenerationAttemptRetry('generation-attempt-second')).toBe(true);
        controller.beginNewGenerationOperation();
        await expect(controller.sendMessage('바뀐 요청')).resolves.toBe(false);
        expect(itemAt(inputs, 4)).not.toHaveProperty('generation_attempt_id');
        expect(nonceOf(itemAt(inputs, 4))).not.toBe(nonceOf(itemAt(inputs, 3)));
        controller.destroy();
    });

    it('binds a restored approved attempt id to the first bounded operation after restart', async () => {
        const inputs: SendMessageInput[] = [];
        const controller = await readyController({
            sendMessage: (input) => {
                inputs.push(structuredClone(input));
                return Promise.reject(permissionDenied());
            },
        });

        expect(controller.stageGenerationAttemptRetry('generation-attempt-restored')).toBe(true);
        await expect(controller.sendMessage('복구한 요청')).resolves.toBe(false);
        await expect(controller.sendMessage('복구한 요청')).resolves.toBe(false);

        expect(inputs).toEqual([
            {
                conversation_id: conversation.id,
                branch_id: branch.id,
                expected_head: null,
                mode: 'chat',
                text: '복구한 요청',
                selection: {
                    kind: 'target',
                    target: {
                        model_route_id: 'route-1',
                        generation_preset_id: 'preset-1',
                    },
                },
                generation_attempt_id: 'generation-attempt-restored',
            },
            {
                conversation_id: conversation.id,
                branch_id: branch.id,
                expected_head: null,
                mode: 'chat',
                text: '복구한 요청',
                selection: {
                    kind: 'target',
                    target: {
                        model_route_id: 'route-1',
                        generation_preset_id: 'preset-1',
                    },
                },
                generation_attempt_id: 'generation-attempt-restored',
            },
        ]);
        controller.destroy();
    });

    it('keys edit and regenerate retries by their exact action inputs', async () => {
        const edits: EditUserMessageInput[] = [];
        const regenerations: RegenerateAssistantMessageInput[] = [];
        const controller = await readyController({
            editUserMessage: (input) => {
                edits.push(structuredClone(input));
                return Promise.reject(permissionDenied());
            },
            regenerateAssistantMessage: (input) => {
                regenerations.push(structuredClone(input));
                return Promise.reject(permissionDenied());
            },
        });

        await expect(controller.editUserMessage('message-user', '  수정안  ')).resolves.toBe(false);
        await expect(controller.editUserMessage('message-user', '수정안')).resolves.toBe(false);
        expect(edits[1]).toEqual(edits[0]);
        expect(edits[0]).toEqual({
            conversation_id: conversation.id,
            branch_id: branch.id,
            expected_head: null,
            message_id: 'message-user',
            replacement_text: '수정안',
            selection: {
                kind: 'target',
                target: { model_route_id: 'route-1', generation_preset_id: 'preset-1' },
            },
            operation_nonce: nonceOf(itemAt(edits, 0)),
        });

        expect(controller.stageGenerationAttemptRetry('generation-attempt-edit')).toBe(true);
        await expect(controller.editUserMessage('message-user', '수정안')).resolves.toBe(false);
        expect(itemAt(edits, 2)).toMatchObject({
            message_id: 'message-user',
            replacement_text: '수정안',
            generation_attempt_id: 'generation-attempt-edit',
        });
        expect(itemAt(edits, 2)).not.toHaveProperty('operation_nonce');

        await expect(controller.editUserMessage('message-user', '다른 수정안')).resolves.toBe(
            false,
        );
        expect(itemAt(edits, 3)).not.toHaveProperty('generation_attempt_id');
        expect(nonceOf(itemAt(edits, 3))).not.toBe(nonceOf(itemAt(edits, 1)));
        expect(get(controller.state).chat.active_generation_id).toBeNull();

        await expect(controller.regenerateAssistantMessage('message-assistant')).resolves.toBe(
            false,
        );
        expect(controller.stageGenerationAttemptRetry('generation-attempt-regenerate')).toBe(true);
        await expect(controller.regenerateAssistantMessage('message-assistant')).resolves.toBe(
            false,
        );
        expect(regenerations[0]).toEqual({
            conversation_id: conversation.id,
            branch_id: branch.id,
            expected_head: null,
            message_id: 'message-assistant',
            selection: {
                kind: 'target',
                target: { model_route_id: 'route-1', generation_preset_id: 'preset-1' },
            },
            operation_nonce: nonceOf(itemAt(regenerations, 0)),
        });
        expect(itemAt(regenerations, 1)).toMatchObject({
            message_id: 'message-assistant',
            generation_attempt_id: 'generation-attempt-regenerate',
        });
        expect(itemAt(regenerations, 1)).not.toHaveProperty('operation_nonce');
        expect(nonceOf(itemAt(regenerations, 0))).not.toBe(nonceOf(itemAt(edits, 3)));
        expect(get(controller.state).chat.active_generation_id).toBeNull();
        controller.destroy();
    });

    it.each(['edit', 'regenerate'] as const)(
        'starts the exact approved %s with attempt-id-only authority',
        async (action) => {
            const inputs: (EditUserMessageInput | RegenerateAssistantMessageInput)[] = [];
            const approvedBranch: ConversationBranchDto = {
                ...branch,
                id: `branch-approved-${action}`,
                fork_message_id: 'message-source',
            };
            const start = (
                input: EditUserMessageInput | RegenerateAssistantMessageInput,
            ): Promise<{ branch: ConversationBranchDto; generation_id: string }> => {
                inputs.push(structuredClone(input));
                return inputs.length === 1
                    ? Promise.reject(permissionDenied())
                    : Promise.resolve({
                          branch: approvedBranch,
                          generation_id: `generation-approved-${action}`,
                      });
            };
            const controller = await readyController({
                editUserMessage: (input) => start(input),
                regenerateAssistantMessage: (input) => start(input),
                selectBranch: (_conversationId, branchId) =>
                    Promise.resolve({ ...conversationState, active_branch_id: branchId }),
            });

            const firstAccepted =
                action === 'edit'
                    ? await controller.editUserMessage('message-source', '승인된 수정')
                    : await controller.regenerateAssistantMessage('message-source');
            expect(firstAccepted).toBe(false);
            expect(nonceOf(itemAt(inputs, 0))).toEqual(expect.any(String));
            expect(controller.stageGenerationAttemptRetry(`attempt-approved-${action}`)).toBe(true);

            const retryAccepted =
                action === 'edit'
                    ? await controller.editUserMessage('message-source', '승인된 수정')
                    : await controller.regenerateAssistantMessage('message-source');

            expect(retryAccepted).toBe(true);
            expect(itemAt(inputs, 1)).toEqual({
                conversation_id: conversation.id,
                branch_id: branch.id,
                expected_head: null,
                message_id: 'message-source',
                ...(action === 'edit' ? { replacement_text: '승인된 수정' } : {}),
                selection: {
                    kind: 'target',
                    target: {
                        model_route_id: 'route-1',
                        generation_preset_id: 'preset-1',
                    },
                },
                generation_attempt_id: `attempt-approved-${action}`,
            });
            expect(itemAt(inputs, 1)).not.toHaveProperty('operation_nonce');
            expect(get(controller.state)).toMatchObject({
                conversation_state: { active_branch_id: approvedBranch.id },
                chat: {
                    phase: 'ready',
                    active_generation_id: `generation-approved-${action}`,
                },
            });
            controller.destroy();
        },
    );

    it('surfaces a restored-attempt input mismatch without reporting a fake start', async () => {
        const inputs: EditUserMessageInput[] = [];
        const controller = await readyController({
            editUserMessage: (input) => {
                inputs.push(structuredClone(input));
                return Promise.reject(
                    Object.assign(new Error('generation attempt input mismatch'), {
                        code: 'invalid_input',
                        message_key: 'error.generation_attempt_input_mismatch',
                        recoverable: true,
                        operation_id: null,
                        field_errors: [],
                    }),
                );
            },
        });

        expect(controller.stageGenerationAttemptRetry('attempt-restored-for-other-action')).toBe(
            true,
        );
        await expect(
            controller.editUserMessage('different-message', 'different-input'),
        ).resolves.toBe(false);

        expect(itemAt(inputs, 0)).toMatchObject({
            message_id: 'different-message',
            replacement_text: 'different-input',
            generation_attempt_id: 'attempt-restored-for-other-action',
        });
        expect(itemAt(inputs, 0)).not.toHaveProperty('operation_nonce');
        expect(get(controller.state).chat).toMatchObject({
            phase: 'error',
            active_generation_id: null,
        });
        controller.destroy();
    });

    it.each(['success', 'failure'] as const)(
        'does not let a stale in-flight %s overwrite the explicitly rotated authority',
        async (outcome) => {
            const inputs: SendMessageInput[] = [];
            const pending: {
                resolve: (value: { generation_id: string }) => void;
                reject: (reason: unknown) => void;
            }[] = [];
            const controller = await readyController({
                sendMessage: (input) => {
                    inputs.push(structuredClone(input));
                    return new Promise((resolve, reject) => pending.push({ resolve, reject }));
                },
            });

            const first = controller.sendMessage('동시 요청');
            controller.beginNewGenerationOperation();
            const second = controller.sendMessage('동시 요청');
            expect(nonceOf(itemAt(inputs, 1))).not.toBe(nonceOf(itemAt(inputs, 0)));

            if (outcome === 'success') {
                itemAt(pending, 0).resolve({ generation_id: 'generation-stale' });
            } else itemAt(pending, 0).reject(permissionDenied());
            await expect(first).resolves.toBe(false);
            itemAt(pending, 1).reject(permissionDenied());
            await expect(second).resolves.toBe(false);

            const retry = controller.sendMessage('동시 요청');
            expect(nonceOf(itemAt(inputs, 2))).toBe(nonceOf(itemAt(inputs, 1)));
            itemAt(pending, 2).reject(permissionDenied());
            await expect(retry).resolves.toBe(false);
            controller.destroy();
        },
    );
});

describe('LorepiaAppController stream lifecycle', () => {
    it('dispatches only an exact retained reviewed prompt and rejects a stale room locally', async () => {
        const sendReviewedPrompt = vi.fn(() =>
            Promise.resolve({ generation_id: 'generation-reviewed' }),
        );
        const controller = new LorepiaAppController(mockClient({ sendReviewedPrompt }));
        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        const input = {
            conversation_id: conversation.id,
            branch_id: branch.id,
            expected_head: branch.head_message_id,
            user_text: '검토한 메시지',
            generation_target: {
                model_route_id: 'route-reviewed',
                generation_preset_id: 'preset-reviewed',
            },
            prompt_preset_id: 'prompt-reviewed',
            variable_overrides: { values: [] },
            expected_plan_hash: 'a'.repeat(64),
            generation_attempt_id: 'generation-attempt-reviewed',
        };

        await expect(controller.sendReviewedPrompt(input)).resolves.toBe(true);
        expect(sendReviewedPrompt).toHaveBeenCalledWith(
            input,
            expect.any(String),
            expect.any(Function),
        );

        controller.destroy();
        const staleController = new LorepiaAppController(mockClient({ sendReviewedPrompt }));
        await staleController.start();
        await staleController.selectCharacter(character);
        await staleController.selectConversation(conversation);
        await expect(
            staleController.sendReviewedPrompt({ ...input, expected_head: 'stale-head' }),
        ).resolves.toBe(false);
        expect(sendReviewedPrompt).toHaveBeenCalledTimes(1);
        staleController.destroy();
    });

    it('uses the Core-resolved room target instead of global settings for send, edit, and regenerate', async () => {
        const roomTarget = {
            model_route_id: 'route-room-b',
            generation_preset_id: 'preset-room-b',
        };
        const selections: GenerationSelectionInput[] = [];
        const sendMessage = vi.fn((input: SendMessageInput) => {
            selections.push(input.selection);
            return Promise.resolve({ generation_id: 'generation-send' });
        });
        const editUserMessage = vi.fn((input: EditUserMessageInput) => {
            selections.push(input.selection);
            return Promise.resolve({ branch, generation_id: 'generation-edit' });
        });
        const regenerateAssistantMessage = vi.fn((input: RegenerateAssistantMessageInput) => {
            selections.push(input.selection);
            return Promise.resolve({ branch, generation_id: 'generation-regenerate' });
        });

        for (const operation of ['send', 'edit', 'regenerate'] as const) {
            const client = mockClient({
                sendMessage,
                editUserMessage,
                regenerateAssistantMessage,
                selectBranch: () => Promise.resolve(conversationState),
            });
            const controller = new LorepiaAppController(client);
            await controller.start();
            await controller.selectCharacter(character);
            await controller.selectConversation(conversation);
            controller.setRoomGenerationTarget(conversation.id, branch.id, roomTarget);

            if (operation === 'send') await controller.sendMessage('방별 대상 전송');
            if (operation === 'edit') {
                await controller.editUserMessage('message-user', '방별 대상 수정');
            }
            if (operation === 'regenerate') {
                await controller.regenerateAssistantMessage('message-assistant');
            }
            controller.destroy();
        }

        expect(selections).toEqual(
            Array.from({ length: 3 }, () => ({ kind: 'target', target: roomTarget })),
        );
    });

    it('sends a retained legacy profile after Core normalizes its route and preset identifiers', async () => {
        const retainedProfile = {
            id: 'legacy-profile-retained',
            display_name: '보존된 레거시 프로필',
            base_url: 'https://synthetic.invalid/v1',
            model: 'synthetic-model',
            timeout_seconds: 30,
        };
        const normalizedLegacySettings = {
            preserve_partial_generations: true,
            selected_provider_profile_id: retainedProfile.id,
            selected_model_route_id: retainedProfile.id,
            selected_generation_preset_id: retainedProfile.id,
        };
        const updateSettings = vi.fn(() => Promise.resolve(normalizedLegacySettings));
        const sentInputs: SendMessageInput[] = [];
        const sendMessage = vi.fn((input: SendMessageInput) => {
            sentInputs.push(structuredClone(input));
            return Promise.resolve({ generation_id: 'generation-legacy' });
        });
        const controller = new LorepiaAppController(
            mockClient({
                getProviderOverview: () =>
                    Promise.resolve({
                        templates: [],
                        connections: [],
                        legacy_profiles: [retainedProfile],
                        settings: {
                            preserve_partial_generations: true,
                            selected_provider_profile_id: null,
                            selected_model_route_id: 'route-modern',
                            selected_generation_preset_id: 'preset-modern',
                        },
                    }),
                credentialStatus: () => Promise.resolve({ status: 'available' }),
                updateSettings,
                sendMessage,
            }),
        );
        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);

        await expect(controller.selectLegacyProviderProfile(retainedProfile.id)).resolves.toBe(
            true,
        );
        expect(get(controller.state).providers.workspace.settings).toEqual(
            normalizedLegacySettings,
        );
        await expect(controller.sendMessage('레거시 자격증으로 전송')).resolves.toBe(true);

        expect(sendMessage).toHaveBeenCalledOnce();
        expect(sentInputs[0]?.selection).toEqual({
            kind: 'legacy_profile',
            provider_profile_id: retainedProfile.id,
        });
        controller.destroy();
    });

    it('disposes the receiver and detaches a late callback after destroy', async () => {
        let onItem: ((item: ChatStreamItemDto) => void) | null = null;
        let streamId: string | null = null;
        const disposeChatStream = vi.fn((streamId: string) => Promise.resolve(streamId.length > 0));
        const client = mockClient({
            sendMessage: (_input, id, listener) => {
                streamId = id;
                onItem = listener;
                return Promise.resolve({ generation_id: 'generation-1' });
            },
            disposeChatStream,
        });
        const controller = new LorepiaAppController(client);

        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        expect(await controller.sendMessage('안녕')).toBe(true);
        expect(onItem).not.toBeNull();

        controller.destroy();
        expect(streamId).not.toBeNull();
        expect(disposeChatStream).toHaveBeenCalledWith(streamId);
        const detachedListener = onItem as unknown as (item: ChatStreamItemDto) => void;
        detachedListener(textEvent(1));

        expect(get(controller.state).chat.streaming_text).toBe('');
    });

    it('batches adjacent text deltas into one short-interval state update', async () => {
        vi.useFakeTimers();
        let onItem: ((item: ChatStreamItemDto) => void) | null = null;
        const client = mockClient({
            sendMessage: (_input, _streamId, listener) => {
                onItem = listener;
                return Promise.resolve({ generation_id: 'generation-1' });
            },
        });
        const controller = new LorepiaAppController(client);
        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        await controller.sendMessage('안녕');

        const listener = onItem as unknown as (item: ChatStreamItemDto) => void;
        listener(textEvent(1, '첫째'));
        listener(textEvent(2, '둘째'));
        expect(get(controller.state).chat.streaming_text).toBe('');

        await vi.advanceTimersByTimeAsync(16);
        expect(get(controller.state).chat.streaming_text).toBe('첫째둘째');
        controller.destroy();
    });

    it('flushes pending reasoning and text synchronously before verified terminal reconciliation', async () => {
        vi.useFakeTimers();
        const terminalSnapshot = deferred<ConversationStateDto>();
        const committed: MessageDto = {
            id: 'message-1',
            conversation_id: conversation.id,
            parent_id: null,
            role: 'assistant',
            content: '저장된 마지막 답변',
            status: 'complete',
            generation_id: 'generation-1',
            created_at: '2026-08-02T00:00:00Z',
        };
        let stateReads = 0;
        let messageReads = 0;
        let onItem: ((item: ChatStreamItemDto) => void) | null = null;
        const client = mockClient({
            getConversationState: () => {
                stateReads += 1;
                return stateReads === 1
                    ? Promise.resolve(conversationState)
                    : terminalSnapshot.promise;
            },
            listBranchMessages: () => {
                messageReads += 1;
                return Promise.resolve(messageReads === 1 ? [] : [committed]);
            },
            sendMessage: (_input, _streamId, listener) => {
                onItem = listener;
                return Promise.resolve({ generation_id: 'generation-1' });
            },
        });
        const controller = new LorepiaAppController(client);
        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        await controller.sendMessage('안녕');

        const listener = onItem as unknown as (item: ChatStreamItemDto) => void;
        listener(reasoningEvent(1, '마지막 추론'));
        listener(textEvent(2, '마지막 답변'));
        listener(terminalEvent(3));

        expect(get(controller.state).chat).toMatchObject({
            streaming_text: '마지막 답변',
            reasoning_text: '마지막 추론',
        });
        await vi.advanceTimersByTimeAsync(16);
        expect(get(controller.state).chat).toMatchObject({
            streaming_text: '마지막 답변',
            reasoning_text: '마지막 추론',
        });

        terminalSnapshot.resolve(conversationState);
        await vi.waitFor(() => expect(get(controller.state).chat.phase).toBe('idle'));
        expect(get(controller.state).messages.items).toEqual([committed]);
        controller.destroy();
    });

    it('still cancels pending deltas when reconciliation is not a verified terminal', async () => {
        vi.useFakeTimers();
        const reconciliationSnapshot = deferred<ConversationStateDto>();
        let stateReads = 0;
        let onItem: ((item: ChatStreamItemDto) => void) | null = null;
        const client = mockClient({
            getConversationState: () => {
                stateReads += 1;
                return stateReads === 1
                    ? Promise.resolve(conversationState)
                    : reconciliationSnapshot.promise;
            },
            sendMessage: (_input, _streamId, listener) => {
                onItem = listener;
                return Promise.resolve({ generation_id: 'generation-1' });
            },
        });
        const controller = new LorepiaAppController(client);
        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        await controller.sendMessage('안녕');

        const listener = onItem as unknown as (item: ChatStreamItemDto) => void;
        listener(reasoningEvent(1, '신뢰할 수 없는 추론'));
        listener({
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
        });
        await vi.advanceTimersByTimeAsync(16);

        expect(get(controller.state).chat.reasoning_text).toBe('');
        reconciliationSnapshot.resolve(conversationState);
        await vi.waitFor(() => expect(stateReads).toBeGreaterThan(1));
        controller.destroy();
    });

    it('disposes the receiver when the stream command fails', async () => {
        let streamId: string | null = null;
        const disposeChatStream = vi.fn((streamId: string) => Promise.resolve(streamId.length > 0));
        const client = mockClient({
            sendMessage: (_input, id) => {
                streamId = id;
                return Promise.reject(new Error('invoke failed'));
            },
            disposeChatStream,
        });
        const controller = new LorepiaAppController(client);

        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);

        expect(await controller.sendMessage('안녕')).toBe(false);
        expect(disposeChatStream).toHaveBeenCalledWith(streamId);
        expect(get(controller.state).chat.phase).toBe('error');
    });

    it('idempotently disposes the stale receiver again after an epoch mismatch', async () => {
        let streamId: string | null = null;
        let resolveStarted: ((value: { generation_id: string }) => void) | null = null;
        const disposeChatStream = vi.fn((streamId: string) => Promise.resolve(streamId.length > 0));
        const client = mockClient({
            sendMessage: (_input, id) => {
                streamId = id;
                return new Promise((resolve) => {
                    resolveStarted = resolve;
                });
            },
            disposeChatStream,
        });
        const controller = new LorepiaAppController(client);

        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        const sending = controller.sendMessage('안녕');
        expect(streamId).not.toBeNull();

        controller.destroy();
        const complete = resolveStarted as unknown as (value: { generation_id: string }) => void;
        complete({ generation_id: 'generation-1' });

        await expect(sending).resolves.toBe(false);
        expect(disposeChatStream.mock.calls.filter(([id]) => id === streamId)).toHaveLength(2);
    });

    it('keeps persisted pending generations blocked when live reattachment is unavailable', async () => {
        const pending: MessageDto = {
            id: 'message-1',
            conversation_id: conversation.id,
            parent_id: null,
            role: 'assistant',
            content: '저장된 부분 응답',
            status: 'pending',
            generation_id: 'generation-1',
            created_at: '2026-08-02T00:00:00Z',
        };
        const subscribeGeneration = vi.fn(() =>
            Promise.reject(
                new LorepiaClientError({
                    code: 'generation_reattachment_unavailable',
                    message_key: 'chat.generation_reattachment_unavailable',
                    recoverable: false,
                    operation_id: 'test-operation',
                    field_errors: [],
                }),
            ),
        );
        const sendMessage = vi.fn(() =>
            Promise.resolve({ generation_id: 'unexpected-generation' }),
        );
        const editUserMessage = vi.fn(() =>
            Promise.resolve({ branch, generation_id: 'unexpected-generation' }),
        );
        const regenerateAssistantMessage = vi.fn(() =>
            Promise.resolve({ branch, generation_id: 'unexpected-generation' }),
        );
        const client = mockClient({
            listBranchMessages: () => Promise.resolve([pending]),
            subscribeGeneration,
            selectBranch: () => Promise.resolve(conversationState),
            sendMessage,
            editUserMessage,
            regenerateAssistantMessage,
        });
        const controller = new LorepiaAppController(client);

        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        await vi.waitFor(() => expect(get(controller.state).chat.phase).toBe('error'));
        expect(subscribeGeneration).toHaveBeenCalledWith(
            'generation-1',
            conversation.id,
            branch.id,
            0,
            expect.any(String),
            expect.any(Function),
        );
        expect(get(controller.state).chat).toMatchObject({
            phase: 'error',
            active_generation_id: 'generation-1',
        });
        expect(get(controller.state).chat.error).toContain('다시 연결할 수 없습니다');
        await expect(controller.sendMessage('새 메시지')).resolves.toBe(false);
        await expect(controller.editUserMessage('message-user', '고친 메시지')).resolves.toBe(
            false,
        );
        await expect(controller.regenerateAssistantMessage('message-1')).resolves.toBe(false);
        expect(sendMessage).not.toHaveBeenCalled();
        expect(editUserMessage).not.toHaveBeenCalled();
        expect(regenerateAssistantMessage).not.toHaveBeenCalled();

        await controller.selectConversation(conversation);
        await controller.selectBranch(branch.id);
        await vi.waitFor(() => expect(subscribeGeneration).toHaveBeenCalledTimes(3));
        controller.destroy();
    });

    it('restores authoritative prefixes before replaying suffixes after live reattachment', async () => {
        const pending: MessageDto = {
            id: 'message-1',
            conversation_id: conversation.id,
            parent_id: null,
            role: 'assistant',
            content: '저장된 부분 응답',
            status: 'pending',
            generation_id: 'generation-1',
            created_at: '2026-08-02T00:00:00Z',
        };
        let listener: ((item: ChatStreamItemDto) => void) | null = null;
        let messageReadCount = 0;
        const subscribeGeneration = vi.fn(
            (
                _generationId: string,
                _conversationId: string,
                _branchId: string,
                sequenceBaseline: number,
                _streamId: string,
                onItem: (item: ChatStreamItemDto) => void,
            ) => {
                onItem({
                    type: 'reconciliation_required',
                    payload: {
                        reason: 'live_snapshot',
                        generation_id: 'generation-1',
                        conversation_id: conversation.id,
                        branch_id: branch.id,
                        last_sequence: sequenceBaseline,
                        observed_sequence: 9,
                        dropped_events: null,
                        supported_event_version: 4,
                        display_prefix: '권위 있는 답변 접두',
                        reasoning_prefix: '권위 있는 추론 접두',
                    },
                });
                onItem(textEvent(10, ' + 답변 접미'));
                onItem(reasoningEvent(11, ' + 추론 접미'));
                return Promise.resolve();
            },
        );
        const disposeChatStream = vi.fn((streamId: string) => Promise.resolve(streamId.length > 0));
        const client = mockClient({
            listBranchMessages: () => Promise.resolve(messageReadCount++ === 0 ? [] : [pending]),
            sendMessage: (_input, _streamId, onItem) => {
                listener = onItem;
                return Promise.resolve({ generation_id: 'generation-1' });
            },
            subscribeGeneration,
            disposeChatStream,
        });
        const controller = new LorepiaAppController(client);

        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        expect(await controller.sendMessage('안녕')).toBe(true);

        const activeListener = listener as unknown as (item: ChatStreamItemDto) => void;
        activeListener({
            type: 'reconciliation_required',
            payload: {
                reason: 'sequence_gap',
                generation_id: 'generation-1',
                conversation_id: conversation.id,
                branch_id: branch.id,
                last_sequence: 0,
                observed_sequence: 7,
                dropped_events: null,
                supported_event_version: 4,
                display_prefix: null,
                reasoning_prefix: null,
            },
        });

        await vi.waitFor(() =>
            expect(subscribeGeneration).toHaveBeenCalledWith(
                'generation-1',
                conversation.id,
                branch.id,
                7,
                expect.any(String),
                expect.any(Function),
            ),
        );
        await vi.waitFor(() => expect(messageReadCount).toBe(2));
        expect(subscribeGeneration).toHaveBeenCalledOnce();
        expect(disposeChatStream).toHaveBeenCalledOnce();
        expect(get(controller.state).chat.phase).toBe('ready');
        expect(get(controller.state).chat.active_generation_id).toBe('generation-1');
        expect(get(controller.state).chat.live_assistant_message_id).toBe(pending.id);
        expect(get(controller.state).messages.items).toEqual([pending]);
        await vi.waitFor(() =>
            expect(get(controller.state).chat).toMatchObject({
                streaming_text: '권위 있는 답변 접두 + 답변 접미',
                reasoning_text: '권위 있는 추론 접두 + 추론 접미',
            }),
        );
        controller.destroy();
    });

    it.each([
        ['route', { conversation_id: 'wrong-conversation' }],
        ['generation', { generation_id: 'wrong-generation' }],
        ['version', { supported_event_version: 999 }],
    ])('fails closed on a reattachment snapshot with the wrong %s', async (_label, overrides) => {
        const pending: MessageDto = {
            id: 'message-1',
            conversation_id: conversation.id,
            parent_id: null,
            role: 'assistant',
            content: '저장된 부분 응답',
            status: 'pending',
            generation_id: 'generation-1',
            created_at: '2026-08-02T00:00:00Z',
        };
        const complete: MessageDto = { ...pending, status: 'complete', content: '완료된 응답' };
        let initialListener: ((item: ChatStreamItemDto) => void) | null = null;
        let messageReadCount = 0;
        const disposeChatStream = vi.fn(() => Promise.resolve(true));
        const subscribeGeneration = vi.fn(
            (
                _generationId: string,
                _conversationId: string,
                _branchId: string,
                sequenceBaseline: number,
                _streamId: string,
                onItem: (item: ChatStreamItemDto) => void,
            ) => {
                onItem({
                    type: 'reconciliation_required',
                    payload: {
                        reason: 'live_snapshot',
                        generation_id: 'generation-1',
                        conversation_id: conversation.id,
                        branch_id: branch.id,
                        last_sequence: sequenceBaseline,
                        observed_sequence: 7,
                        dropped_events: null,
                        supported_event_version: 4,
                        display_prefix: '검증되지 않은 접두',
                        reasoning_prefix: '',
                        ...overrides,
                    },
                });
                onItem(textEvent(8, '표시되면 안 되는 접미'));
                return Promise.resolve();
            },
        );
        const controller = new LorepiaAppController(
            mockClient({
                listBranchMessages: () => {
                    messageReadCount += 1;
                    if (messageReadCount === 1) return Promise.resolve([]);
                    if (messageReadCount === 2) return Promise.resolve([pending]);
                    return Promise.resolve([complete]);
                },
                sendMessage: (_input, _streamId, onItem) => {
                    initialListener = onItem;
                    return Promise.resolve({ generation_id: 'generation-1' });
                },
                subscribeGeneration,
                disposeChatStream,
            }),
        );
        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        expect(await controller.sendMessage('안녕')).toBe(true);

        const activeInitialListener = initialListener as unknown as (
            item: ChatStreamItemDto,
        ) => void;
        activeInitialListener({
            type: 'reconciliation_required',
            payload: {
                reason: 'broadcast_lagged',
                generation_id: 'generation-1',
                conversation_id: conversation.id,
                branch_id: branch.id,
                last_sequence: 0,
                observed_sequence: null,
                dropped_events: 1,
                supported_event_version: 4,
                display_prefix: null,
                reasoning_prefix: null,
            },
        });

        await vi.waitFor(() => expect(get(controller.state).chat.phase).toBe('idle'));
        expect(disposeChatStream).toHaveBeenCalledTimes(2);
        expect(get(controller.state).chat.streaming_text).toBe('');
        expect(get(controller.state).chat.live_assistant_message_id).toBeNull();
        expect(get(controller.state).messages.items).toEqual([complete]);
        controller.destroy();
    });

    it('replays a terminal event buffered immediately after the atomic live snapshot', async () => {
        const pending: MessageDto = {
            id: 'message-1',
            conversation_id: conversation.id,
            parent_id: null,
            role: 'assistant',
            content: '저장된 부분 응답',
            status: 'pending',
            generation_id: 'generation-1',
            created_at: '2026-08-02T00:00:00Z',
        };
        const complete: MessageDto = { ...pending, status: 'complete', content: '완료된 응답' };
        const terminalSnapshot = deferred<ConversationStateDto>();
        let initialListener: ((item: ChatStreamItemDto) => void) | null = null;
        let stateReadCount = 0;
        let messageReadCount = 0;
        const subscribeGeneration = vi.fn(
            (
                _generationId: string,
                _conversationId: string,
                _branchId: string,
                sequenceBaseline: number,
                _streamId: string,
                onItem: (item: ChatStreamItemDto) => void,
            ) => {
                onItem({
                    type: 'reconciliation_required',
                    payload: {
                        reason: 'live_snapshot',
                        generation_id: 'generation-1',
                        conversation_id: conversation.id,
                        branch_id: branch.id,
                        last_sequence: sequenceBaseline,
                        observed_sequence: 9,
                        dropped_events: null,
                        supported_event_version: 4,
                        display_prefix: '권위 있는 답변 접두',
                        reasoning_prefix: '권위 있는 추론 접두',
                    },
                });
                onItem({
                    type: 'event',
                    payload: {
                        event_version: 4,
                        generation_id: 'generation-1',
                        conversation_id: conversation.id,
                        branch_id: branch.id,
                        assistant_message_id: 'message-1',
                        sequence: 10,
                        emitted_at: '2026-08-02T00:00:01Z',
                        kind: { type: 'generation_finished' },
                    },
                });
                return Promise.resolve();
            },
        );
        const client = mockClient({
            getConversationState: () => {
                stateReadCount += 1;
                return stateReadCount === 3
                    ? terminalSnapshot.promise
                    : Promise.resolve(conversationState);
            },
            listBranchMessages: () => {
                messageReadCount += 1;
                if (messageReadCount === 1) return Promise.resolve([]);
                if (messageReadCount === 2) return Promise.resolve([pending]);
                return Promise.resolve([complete]);
            },
            sendMessage: (_input, _streamId, onItem) => {
                initialListener = onItem;
                return Promise.resolve({ generation_id: 'generation-1' });
            },
            subscribeGeneration,
        });
        const controller = new LorepiaAppController(client);

        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        expect(await controller.sendMessage('안녕')).toBe(true);
        const activeInitialListener = initialListener as unknown as (
            item: ChatStreamItemDto,
        ) => void;
        activeInitialListener({
            type: 'reconciliation_required',
            payload: {
                reason: 'broadcast_lagged',
                generation_id: 'generation-1',
                conversation_id: conversation.id,
                branch_id: branch.id,
                last_sequence: 7,
                observed_sequence: null,
                dropped_events: 1,
                supported_event_version: 4,
                display_prefix: null,
                reasoning_prefix: null,
            },
        });
        await vi.waitFor(() => expect(stateReadCount).toBe(3));
        expect(get(controller.state).chat.live_assistant_message_id).toBe(pending.id);
        terminalSnapshot.resolve(conversationState);

        await vi.waitFor(() => expect(get(controller.state).chat.phase).toBe('idle'));
        expect(subscribeGeneration).toHaveBeenCalledOnce();
        expect(messageReadCount).toBe(3);
        expect(get(controller.state).chat.active_generation_id).toBeNull();
        expect(get(controller.state).chat.live_assistant_message_id).toBeNull();
        expect(get(controller.state).messages.items).toEqual([complete]);
        controller.destroy();
    });

    it('settles a terminal commit that wins the pending-to-subscribe race', async () => {
        const pending: MessageDto = {
            id: 'message-1',
            conversation_id: conversation.id,
            parent_id: null,
            role: 'assistant',
            content: '저장된 부분 응답',
            status: 'pending',
            generation_id: 'generation-1',
            created_at: '2026-08-02T00:00:00Z',
        };
        const complete: MessageDto = { ...pending, status: 'complete', content: '완료된 응답' };
        let listener: ((item: ChatStreamItemDto) => void) | null = null;
        let messageReadCount = 0;
        const subscribeGeneration = vi.fn(() =>
            Promise.reject(
                new LorepiaClientError({
                    code: 'generation_reattachment_unavailable',
                    message_key: 'chat.generation_reattachment_unavailable',
                    recoverable: false,
                    operation_id: 'terminal-race',
                    field_errors: [],
                }),
            ),
        );
        const client = mockClient({
            listBranchMessages: () => {
                messageReadCount += 1;
                if (messageReadCount === 1) return Promise.resolve([]);
                if (messageReadCount === 2) return Promise.resolve([pending]);
                return Promise.resolve([complete]);
            },
            sendMessage: (_input, _streamId, onItem) => {
                listener = onItem;
                return Promise.resolve({ generation_id: 'generation-1' });
            },
            subscribeGeneration,
        });
        const controller = new LorepiaAppController(client);

        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        expect(await controller.sendMessage('안녕')).toBe(true);
        const activeListener = listener as unknown as (item: ChatStreamItemDto) => void;
        activeListener({
            type: 'reconciliation_required',
            payload: {
                reason: 'broadcast_lagged',
                generation_id: 'generation-1',
                conversation_id: conversation.id,
                branch_id: branch.id,
                last_sequence: 7,
                observed_sequence: null,
                dropped_events: 1,
                supported_event_version: 4,
                display_prefix: null,
                reasoning_prefix: null,
            },
        });

        await vi.waitFor(() => expect(get(controller.state).chat.phase).toBe('idle'));
        expect(subscribeGeneration).toHaveBeenCalledOnce();
        expect(messageReadCount).toBe(3);
        expect(get(controller.state).chat.active_generation_id).toBeNull();
        expect(get(controller.state).messages.items).toEqual([complete]);
        controller.destroy();
    });

    it('reconciles a closed stream or marker received before the send response', async () => {
        const earlyItems: ChatStreamItemDto[] = [
            { type: 'closed' },
            {
                type: 'reconciliation_required',
                payload: {
                    reason: 'sequence_gap',
                    generation_id: 'generation-1',
                    conversation_id: conversation.id,
                    branch_id: branch.id,
                    last_sequence: 0,
                    observed_sequence: 2,
                    dropped_events: null,
                    supported_event_version: 4,
                    display_prefix: null,
                    reasoning_prefix: null,
                },
            },
        ];

        for (const earlyItem of earlyItems) {
            const disposeChatStream = vi.fn((streamId: string) =>
                Promise.resolve(streamId.length > 0),
            );
            const client = mockClient({
                sendMessage: (_input, _streamId, listener) => {
                    listener(earlyItem);
                    return Promise.resolve({ generation_id: 'generation-1' });
                },
                disposeChatStream,
            });
            const controller = new LorepiaAppController(client);

            await controller.start();
            await controller.selectCharacter(character);
            await controller.selectConversation(conversation);
            expect(await controller.sendMessage('안녕')).toBe(true);

            await vi.waitFor(() => expect(get(controller.state).chat.phase).toBe('idle'));
            expect(get(controller.state).chat.active_generation_id).toBeNull();
            expect(disposeChatStream).toHaveBeenCalledTimes(1);
            controller.destroy();
        }
    });

    it('uses the refreshed branch head for the next send after terminal reconciliation', async () => {
        const refreshedBranch: ConversationBranchDto = {
            ...branch,
            head_message_id: 'message-committed',
            updated_at: '2026-08-02T00:00:01Z',
        };
        const sentHeads: (string | null)[] = [];
        const listeners: ((item: ChatStreamItemDto) => void)[] = [];
        const listBranches = vi
            .fn<() => Promise<ConversationBranchDto[]>>()
            .mockResolvedValueOnce([branch])
            .mockResolvedValue([refreshedBranch]);
        const client = mockClient({
            listBranches,
            sendMessage: (input, _streamId, listener) => {
                sentHeads.push(input.expected_head);
                listeners.push(listener);
                return Promise.resolve({
                    generation_id: `generation-${String(sentHeads.length)}`,
                });
            },
        });
        const controller = new LorepiaAppController(client);

        await controller.start();
        await controller.selectCharacter(character);
        await controller.selectConversation(conversation);
        expect(await controller.sendMessage('첫 메시지')).toBe(true);

        listeners[0]?.({
            type: 'event',
            payload: {
                event_version: 4,
                generation_id: 'generation-1',
                conversation_id: conversation.id,
                branch_id: branch.id,
                assistant_message_id: 'message-1',
                sequence: 1,
                emitted_at: '2026-08-02T00:00:01Z',
                kind: { type: 'generation_finished' },
            },
        });

        await vi.waitFor(() =>
            expect(get(controller.state).branches[0]?.head_message_id).toBe('message-committed'),
        );
        expect(get(controller.state).chat.phase).toBe('idle');
        expect(await controller.sendMessage('다음 메시지')).toBe(true);
        expect(sentHeads).toEqual([null, 'message-committed']);
        controller.destroy();
    });
});
