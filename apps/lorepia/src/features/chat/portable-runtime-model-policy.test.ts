// @vitest-environment node

import { afterEach, describe, expect, it } from 'vitest';

import {
    MAX_PORTABLE_RUNTIME_MODEL_OUTPUT_TOKENS,
    MAX_PORTABLE_RUNTIME_MODEL_SESSION_TOKENS,
    beginPortableRuntimeModelCall,
    portableRuntimeModelBudgetSnapshot,
    resetPortableRuntimeModelBudgetsForTests,
} from './portable-runtime-model-policy';

afterEach(() => resetPortableRuntimeModelBudgetsForTests());

describe('portable runtime model budget', () => {
    it('enforces one concurrent call and three starts per rolling minute', () => {
        const first = beginPortableRuntimeModelCall('card:revision', 10, 1_000);
        expect(first.ok).toBe(true);
        expect(beginPortableRuntimeModelCall('card:revision', 10, 1_000)).toEqual({
            ok: false,
            reason: 'concurrent_call',
        });
        if (!first.ok) throw new Error('first call should be admitted');
        first.lease.finish(null);

        for (const now of [2_000, 3_000]) {
            const admitted = beginPortableRuntimeModelCall('card:revision', 10, now);
            expect(admitted.ok).toBe(true);
            if (admitted.ok) admitted.lease.finish(null);
        }
        expect(beginPortableRuntimeModelCall('card:revision', 10, 4_000)).toEqual({
            ok: false,
            reason: 'rate_limit',
        });
        expect(beginPortableRuntimeModelCall('card:revision', 10, 61_001).ok).toBe(true);
    });

    it('reserves output tokens and keeps the budget across runtime instances', () => {
        const promptBytes =
            MAX_PORTABLE_RUNTIME_MODEL_SESSION_TOKENS - MAX_PORTABLE_RUNTIME_MODEL_OUTPUT_TOKENS;
        const admitted = beginPortableRuntimeModelCall('card:revision', promptBytes, 1_000);
        expect(admitted.ok).toBe(true);
        if (!admitted.ok) throw new Error('call should be admitted');
        admitted.lease.finish(null);

        expect(portableRuntimeModelBudgetSnapshot('card:revision').tokensRemaining).toBe(0);
        expect(beginPortableRuntimeModelCall('card:revision', 1, 62_000)).toEqual({
            ok: false,
            reason: 'token_budget',
        });
        expect(beginPortableRuntimeModelCall('different-card:revision', 1, 62_000).ok).toBe(true);
    });

    it('reconciles conservative reservations and enforces the session call ceiling', () => {
        const first = beginPortableRuntimeModelCall('card:revision', 1_000, 1_000);
        if (!first.ok) throw new Error('first call should be admitted');
        first.lease.finish({
            input_tokens: 12,
            cached_read_tokens: null,
            cached_write_tokens: null,
            output_tokens: 8,
            reasoning_tokens: 4,
            tool_tokens: 2,
        });
        expect(portableRuntimeModelBudgetSnapshot('card:revision').chargedTokens).toBe(26);

        for (let index = 1; index < 20; index += 1) {
            const admitted = beginPortableRuntimeModelCall(
                'card:revision',
                0,
                1_000 + index * 61_000,
            );
            expect(admitted.ok).toBe(true);
            if (admitted.ok) {
                admitted.lease.finish({
                    input_tokens: 0,
                    cached_read_tokens: null,
                    cached_write_tokens: null,
                    output_tokens: 0,
                    reasoning_tokens: null,
                    tool_tokens: null,
                });
            }
        }
        expect(beginPortableRuntimeModelCall('card:revision', 0, 2_000_000)).toEqual({
            ok: false,
            reason: 'call_budget',
        });
    });

    it('quarantines a card revision after an outcome-unknown provider call', () => {
        const admitted = beginPortableRuntimeModelCall('card:revision', 128, 1_000);
        if (!admitted.ok) throw new Error('call should be admitted');

        const snapshot = admitted.lease.finish(null, 'unknown_outcome');

        expect(snapshot.blockedByUnknownOutcome).toBe(true);
        expect(snapshot.callsRemaining).toBe(0);
        expect(beginPortableRuntimeModelCall('card:revision', 1, 62_000)).toEqual({
            ok: false,
            reason: 'unknown_outcome',
        });
        expect(beginPortableRuntimeModelCall('other-card:revision', 1, 62_000).ok).toBe(true);
    });

    it('does not quarantine a known no-side-effect failure', () => {
        const admitted = beginPortableRuntimeModelCall('card:revision', 128, 1_000);
        if (!admitted.ok) throw new Error('call should be admitted');

        const snapshot = admitted.lease.finish(null, 'known_failure');

        expect(snapshot.blockedByUnknownOutcome).toBe(false);
        expect(beginPortableRuntimeModelCall('card:revision', 1, 62_000).ok).toBe(true);
    });
});
