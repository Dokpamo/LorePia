import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { MemorySupervisorStatusDto } from '../../lib/ipc/contracts';
import { LorepiaAppController } from '../app-controller';
import { createAppControllerFixture } from './app-controller-test-support';

const { character, mockClient } = createAppControllerFixture();

afterEach(() => {
    vi.useRealTimers();
});

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
