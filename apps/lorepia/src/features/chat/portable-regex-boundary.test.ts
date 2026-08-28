import { describe, expect, it, vi } from 'vitest';

import displaySource from './portable-display.ts?raw';
import { runPortableRegex, setPortableRegexWorkerFactoryForTests } from './portable-regex';
import runtimeSource from './portable-runtime.ts?raw';
import workerSource from './portable-regex.worker.ts?raw';

describe('portable regex isolation boundary', () => {
    it('keeps imported regular expressions out of renderer and runtime modules', () => {
        expect(displaySource).not.toContain('new RegExp(');
        expect(runtimeSource).not.toContain('new RegExp(');
        expect(workerSource).toContain('performPortableRegexOperation');
    });

    it('terminates a worker that misses its deadline', async () => {
        const terminate = vi.fn();
        const restore = setPortableRegexWorkerFactoryForTests(
            () =>
                ({
                    addEventListener: vi.fn(),
                    removeEventListener: vi.fn(),
                    postMessage: vi.fn(),
                    terminate,
                }) as unknown as Worker,
        );
        try {
            await expect(
                runPortableRegex(
                    { operation: 'test', source: 'a'.repeat(32), pattern: '(a+)+$', flags: '' },
                    5,
                ),
            ).resolves.toEqual({ ok: false, timedOut: true });
            expect(terminate).toHaveBeenCalledOnce();
        } finally {
            restore();
        }
    });

    it('never starts more than four regex workers concurrently', async () => {
        let activeWorkers = 0;
        let maximumActiveWorkers = 0;
        const restore = setPortableRegexWorkerFactoryForTests(() => {
            activeWorkers += 1;
            maximumActiveWorkers = Math.max(maximumActiveWorkers, activeWorkers);
            return {
                addEventListener: vi.fn(),
                removeEventListener: vi.fn(),
                postMessage: vi.fn(),
                terminate: () => {
                    activeWorkers -= 1;
                },
            } as unknown as Worker;
        });
        try {
            await Promise.all(
                Array.from({ length: 20 }, (_, index) =>
                    runPortableRegex(
                        {
                            operation: 'test',
                            source: `source-${String(index)}`,
                            pattern: '(a+)+$',
                            flags: '',
                        },
                        5,
                    ),
                ),
            );
            expect(maximumActiveWorkers).toBeLessThanOrEqual(4);
            expect(activeWorkers).toBe(0);
        } finally {
            restore();
        }
    });
});
