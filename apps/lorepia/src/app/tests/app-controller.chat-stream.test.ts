import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
    ChatStreamItemDto,
    ConversationBranchDto,
    ConversationStateDto,
    EditUserMessageInput,
    GenerationSelectionInput,
    MessageDto,
    RegenerateAssistantMessageInput,
    SendMessageInput,
} from '../../lib/ipc/contracts';
import { LorepiaClientError } from '../../lib/ipc/errors';
import { LorepiaAppController } from '../app-controller';
import { createAppControllerFixture, deferred } from './app-controller-test-support';

const {
    character,
    conversation,
    conversationState,
    branch,
    mockClient,
    textEvent,
    reasoningEvent,
    terminalEvent,
} = createAppControllerFixture();

afterEach(() => {
    vi.useRealTimers();
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
