import { get } from 'svelte/store';
import { cleanup } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
    LorepiaClient,
    SaveRoomOrchestrationConfigInput,
    SaveRoomOrchestrationConfigResult,
} from '../../../lib/ipc/contracts';
import { OrchestrationController } from '../orchestration-controller';
import { liveStudioSnapshot } from './live-studio-fixtures';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

function deferred<Value>(): {
    promise: Promise<Value>;
    resolve: (value: Value) => void;
} {
    let resolve!: (value: Value) => void;
    const promise = new Promise<Value>((accept) => {
        resolve = accept;
    });
    return { promise, resolve };
}

describe('OrchestrationController room prompt sources', () => {
    it('preserves a newer draft across a deferred save and advances the next CAS revision', async () => {
        const snapshot = liveStudioSnapshot();
        const firstSave = deferred<SaveRoomOrchestrationConfigResult>();
        const inputs: SaveRoomOrchestrationConfigInput[] = [];
        const saveRoomOrchestrationConfig = vi.fn(
            (
                input: SaveRoomOrchestrationConfigInput,
            ): Promise<SaveRoomOrchestrationConfigResult> => {
                inputs.push(structuredClone(input));
                if (inputs.length === 1) return firstSave.promise;
                const { expected_revision: _expectedRevision, ...roomValues } = input;
                void _expectedRevision;
                return Promise.resolve({
                    room_config: {
                        ...snapshot.room_config,
                        ...roomValues,
                    },
                    revision: 6,
                    generation_target: {
                        model_route_id: 'route-saved',
                        generation_preset_id: 'generation-saved',
                    },
                });
            },
        );
        const orchestrationController = new OrchestrationController({
            getOrchestrationWorkspace: vi.fn().mockResolvedValue(snapshot),
            saveRoomOrchestrationConfig,
        } as unknown as LorepiaClient);

        await orchestrationController.loadContext('conversation-1', 'branch-1');
        orchestrationController.stageRoomConfig({
            user_name_override: '별이',
            author_note: '첫 저장 초안',
            group_context: '별이와 달이 함께 대화한다.',
            template_slots: [{ name: 'tone', value: '차분하게' }],
        });
        const pendingResult = orchestrationController.saveRoomConfig();
        orchestrationController.stageRoomConfig({
            author_note: '저장 중 작성한 더 새로운 초안',
            template_slots: [{ name: 'tone', value: '조금 더 밝게' }],
        });

        const firstInput = inputs[0];
        if (firstInput === undefined) throw new Error('first room save was not dispatched');
        const { expected_revision: _expectedRevision, ...firstRoomValues } = firstInput;
        void _expectedRevision;
        firstSave.resolve({
            room_config: {
                ...snapshot.room_config,
                ...firstRoomValues,
            },
            revision: 5,
            generation_target: {
                model_route_id: 'route-saved',
                generation_preset_id: 'generation-saved',
            },
        });

        await expect(pendingResult).resolves.toBe(true);
        const afterDeferredSave = get(orchestrationController.state);
        expect(inputs[0]).toMatchObject({
            expected_revision: 4,
            user_name_override: '별이',
            author_note: '첫 저장 초안',
            group_context: '별이와 달이 함께 대화한다.',
            template_slots: [{ name: 'tone', value: '차분하게' }],
        });
        expect(afterDeferredSave.workspace.room_config).toMatchObject({
            user_name_override: '별이',
            author_note: '저장 중 작성한 더 새로운 초안',
            group_context: '별이와 달이 함께 대화한다.',
            template_slots: [{ name: 'tone', value: '조금 더 밝게' }],
        });
        expect(afterDeferredSave.workspace.room_config_revision).toBe(5);
        expect(afterDeferredSave.workspace.generation_target).toEqual({
            model_route_id: 'route-saved',
            generation_preset_id: 'generation-saved',
        });
        expect(afterDeferredSave.dirty_room_config).toBe(true);

        await expect(orchestrationController.saveRoomConfig()).resolves.toBe(true);
        expect(inputs[1]).toMatchObject({
            expected_revision: 5,
            user_name_override: '별이',
            author_note: '저장 중 작성한 더 새로운 초안',
            group_context: '별이와 달이 함께 대화한다.',
            template_slots: [{ name: 'tone', value: '조금 더 밝게' }],
        });
        expect(get(orchestrationController.state).dirty_room_config).toBe(false);
    });
});
