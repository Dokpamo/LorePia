import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
    ConversationBranchDto,
    EditUserMessageInput,
    LorepiaClient,
    RegenerateAssistantMessageInput,
    SendMessageInput,
} from '../../lib/ipc/contracts';
import { LorepiaAppController } from '../app-controller';
import { createAppControllerFixture } from './app-controller-test-support';

const { character, conversation, conversationState, branch, mockClient } =
    createAppControllerFixture();

afterEach(() => {
    vi.useRealTimers();
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
