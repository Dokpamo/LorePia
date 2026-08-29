import { afterEach, describe, expect, it, vi } from 'vitest';

import displaySource from './portable-display.ts?raw';
import {
    inspectPortableRegexRules,
    portableRegexRuleKey,
    resetPortableRegexRuleFailuresForTests,
    runPortableRegex,
    setPortableRegexWorkerFactoryForTests,
} from './portable-regex';
import runtimeSource from './portable-runtime.ts?raw';
import workerSource from './portable-regex.worker.ts?raw';

describe('portable regex isolation boundary', () => {
    afterEach(() => resetPortableRegexRuleFailuresForTests());

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
            ).resolves.toEqual({
                ok: false,
                reason: 'execution_timeout',
                timedOut: true,
            });
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

    it('disables only the exact rule signature after an execution timeout', async () => {
        const createWorker = vi.fn(
            () =>
                ({
                    addEventListener: vi.fn(),
                    removeEventListener: vi.fn(),
                    postMessage: vi.fn(),
                    terminate: vi.fn(),
                }) as unknown as Worker,
        );
        const restore = setPortableRegexWorkerFactoryForTests(createWorker);
        try {
            const dangerous = {
                operation: 'test' as const,
                source: 'a'.repeat(32),
                pattern: '(a+)+$',
                flags: '',
            };
            const firstRuleKey = portableRegexRuleKey(
                'first-card:revision',
                'display',
                'rule-1',
                0,
            );
            await expect(
                runPortableRegex(dangerous, { timeoutMs: 5, ruleKey: firstRuleKey }),
            ).resolves.toMatchObject({ reason: 'execution_timeout' });
            await expect(
                runPortableRegex(dangerous, { timeoutMs: 5, ruleKey: firstRuleKey }),
            ).resolves.toEqual({
                ok: false,
                reason: 'disabled',
                disabled: true,
                disabledReason: 'execution_timeout',
            });

            expect(createWorker).toHaveBeenCalledOnce();
            await expect(
                runPortableRegex(dangerous, {
                    timeoutMs: 5,
                    ruleKey: portableRegexRuleKey(
                        'different-card:revision',
                        'display',
                        'rule-1',
                        0,
                    ),
                }),
            ).resolves.toMatchObject({ reason: 'execution_timeout' });
            expect(createWorker).toHaveBeenCalledTimes(2);
        } finally {
            restore();
        }
    });

    it('compiles every reviewed rule independently before import commit', async () => {
        await expect(
            inspectPortableRegexRules(
                [
                    {
                        id: 'compatible',
                        phase: 'display',
                        runtime_index: 0,
                        pattern: '(?<=a)b',
                        flags: 'u',
                    },
                    {
                        id: 'invalid',
                        phase: 'lore',
                        runtime_index: 1,
                        pattern: '(',
                        flags: '',
                    },
                    {
                        id: 'later',
                        phase: 'display',
                        runtime_index: 2,
                        pattern: '(a)(b)',
                        flags: 'g',
                    },
                ],
                'import:inspection-1',
            ),
        ).resolves.toEqual([
            { id: 'compatible', status: 'valid' },
            { id: 'invalid', status: 'invalid' },
            { id: 'later', status: 'valid' },
        ]);
    });

    it('includes worker queue time in each operation deadline', async () => {
        const restore = setPortableRegexWorkerFactoryForTests(
            () =>
                ({
                    addEventListener: vi.fn(),
                    removeEventListener: vi.fn(),
                    postMessage: vi.fn(),
                    terminate: vi.fn(),
                }) as unknown as Worker,
        );
        try {
            const blockers = Array.from({ length: 4 }, () =>
                runPortableRegex({ operation: 'test', source: 'a', pattern: 'a', flags: '' }, 30),
            );
            await expect(
                runPortableRegex({ operation: 'test', source: 'a', pattern: 'a', flags: '' }, 5),
            ).resolves.toEqual({ ok: false, reason: 'queue_timeout', timedOut: true });
            await Promise.all(blockers);
        } finally {
            restore();
        }
    });
});
