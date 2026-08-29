import { describe, expect, it } from 'vitest';

import {
    isPortableRuntimeMainMessage,
    isPortableRuntimeWorkerMessage,
    portableRuntimeMessageWithinLimit,
} from './portable-runtime-protocol';

describe('portable runtime worker protocol', () => {
    it('rejects malformed operations before they reach the worker kernel', () => {
        expect(
            isPortableRuntimeMainMessage({
                channel: 'lorepia-portable-runtime-v1',
                type: 'request',
                requestId: 'runtime-1',
                operation: { type: 'invoke', name: 'arbitrary', values: [], context: {} },
            }),
        ).toBe(false);
    });

    it('accepts only typed success and failure responses', () => {
        expect(
            isPortableRuntimeWorkerMessage({
                channel: 'lorepia-portable-runtime-v1',
                type: 'response',
                requestId: 'runtime-1',
                ok: true,
                result: { type: 'invoked', value: false },
                snapshot: {
                    persisted: {},
                    virtualMessage: null,
                    stopped: true,
                },
            }),
        ).toBe(true);
        expect(
            isPortableRuntimeWorkerMessage({
                channel: 'lorepia-portable-runtime-v1',
                type: 'response',
                requestId: 'runtime-1',
                ok: false,
                error: { code: 'unexpected', message: 'bad' },
            }),
        ).toBe(false);
    });

    it('enforces the aggregate structured-message budget', () => {
        expect(portableRuntimeMessageWithinLimit({ value: 'x'.repeat(8 * 1024 * 1024) })).toBe(
            false,
        );
    });
});
