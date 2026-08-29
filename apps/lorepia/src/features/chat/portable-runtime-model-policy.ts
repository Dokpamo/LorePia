import type { GenerationUsageDto } from '../../lib/ipc/contracts';

export const MAX_PORTABLE_RUNTIME_MODEL_CALLS_PER_MINUTE = 3;
export const MAX_PORTABLE_RUNTIME_MODEL_CALLS_PER_SESSION = 20;
export const MAX_PORTABLE_RUNTIME_MODEL_SESSION_TOKENS = 32_768;
export const MAX_PORTABLE_RUNTIME_MODEL_OUTPUT_TOKENS = 1_024;

const MODEL_RATE_WINDOW_MS = 60_000;

export type PortableRuntimeModelBudgetDenial =
    'concurrent_call' | 'rate_limit' | 'call_budget' | 'token_budget' | 'unknown_outcome';

export type PortableRuntimeModelCallOutcome = 'completed' | 'known_failure' | 'unknown_outcome';

interface PortableRuntimeModelBudgetState {
    callCount: number;
    chargedTokens: number;
    callStartedAt: number[];
    activeRequestId: string | null;
    blockedByUnknownOutcome: boolean;
}

export interface PortableRuntimeModelBudgetSnapshot {
    callCount: number;
    chargedTokens: number;
    callsRemaining: number;
    tokensRemaining: number;
    blockedByUnknownOutcome: boolean;
}

export interface PortableRuntimeModelCallLease {
    requestId: string;
    reservedTokens: number;
    finish(
        usage: GenerationUsageDto | null,
        outcome?: PortableRuntimeModelCallOutcome,
    ): PortableRuntimeModelBudgetSnapshot;
}

export type PortableRuntimeModelBudgetAdmission =
    | { ok: true; lease: PortableRuntimeModelCallLease }
    | { ok: false; reason: PortableRuntimeModelBudgetDenial };

const sessionBudgets = new Map<string, PortableRuntimeModelBudgetState>();

export function beginPortableRuntimeModelCall(
    scope: string,
    promptByteLength: number,
    now = Date.now(),
): PortableRuntimeModelBudgetAdmission {
    const state = sessionBudgets.get(scope) ?? {
        callCount: 0,
        chargedTokens: 0,
        callStartedAt: [],
        activeRequestId: null,
        blockedByUnknownOutcome: false,
    };
    state.callStartedAt = state.callStartedAt.filter(
        (startedAt) => now - startedAt < MODEL_RATE_WINDOW_MS,
    );
    if (state.blockedByUnknownOutcome) return { ok: false, reason: 'unknown_outcome' };
    if (state.activeRequestId !== null) return { ok: false, reason: 'concurrent_call' };
    if (state.callStartedAt.length >= MAX_PORTABLE_RUNTIME_MODEL_CALLS_PER_MINUTE) {
        return { ok: false, reason: 'rate_limit' };
    }
    if (state.callCount >= MAX_PORTABLE_RUNTIME_MODEL_CALLS_PER_SESSION) {
        return { ok: false, reason: 'call_budget' };
    }
    const reservedTokens =
        Math.max(0, Math.ceil(promptByteLength)) + MAX_PORTABLE_RUNTIME_MODEL_OUTPUT_TOKENS;
    if (state.chargedTokens + reservedTokens > MAX_PORTABLE_RUNTIME_MODEL_SESSION_TOKENS) {
        return { ok: false, reason: 'token_budget' };
    }

    const requestId = globalThis.crypto.randomUUID();
    state.callCount += 1;
    state.chargedTokens += reservedTokens;
    state.callStartedAt.push(now);
    state.activeRequestId = requestId;
    sessionBudgets.set(scope, state);
    let finished = false;
    return {
        ok: true,
        lease: {
            requestId,
            reservedTokens,
            finish(usage, outcome = 'completed') {
                if (!finished) {
                    finished = true;
                    const actualTokens = completeUsageTokens(usage) ?? reservedTokens;
                    state.chargedTokens = Math.max(
                        0,
                        state.chargedTokens - reservedTokens + actualTokens,
                    );
                    if (outcome === 'unknown_outcome') state.blockedByUnknownOutcome = true;
                    if (state.activeRequestId === requestId) state.activeRequestId = null;
                }
                return budgetSnapshot(state);
            },
        },
    };
}

export function portableRuntimeModelBudgetSnapshot(
    scope: string,
): PortableRuntimeModelBudgetSnapshot {
    return budgetSnapshot(
        sessionBudgets.get(scope) ?? {
            callCount: 0,
            chargedTokens: 0,
            callStartedAt: [],
            activeRequestId: null,
            blockedByUnknownOutcome: false,
        },
    );
}

function completeUsageTokens(usage: GenerationUsageDto | null): number | null {
    const inputTokens = usage?.input_tokens ?? null;
    const cachedReadTokens = usage?.cached_read_tokens ?? 0;
    const cachedWriteTokens = usage?.cached_write_tokens ?? 0;
    const outputTokens = usage?.output_tokens ?? null;
    const reasoningTokens = usage?.reasoning_tokens ?? 0;
    const toolTokens = usage?.tool_tokens ?? 0;
    if (inputTokens === null || outputTokens === null) return null;
    // Provider cache counters may overlap with input_tokens or represent
    // external cached context that was absent from the local reservation.
    // Summing every reported counter is intentionally conservative: budget
    // enforcement may overcharge ambiguous usage, but it must never let a
    // cache-heavy provider call bypass the session ceiling.
    return [
        inputTokens,
        cachedReadTokens,
        cachedWriteTokens,
        outputTokens,
        reasoningTokens,
        toolTokens,
    ].reduce((total, value) => total + Math.max(0, value), 0);
}

function budgetSnapshot(
    state: PortableRuntimeModelBudgetState,
): PortableRuntimeModelBudgetSnapshot {
    return {
        callCount: state.callCount,
        chargedTokens: state.chargedTokens,
        callsRemaining: state.blockedByUnknownOutcome
            ? 0
            : Math.max(0, MAX_PORTABLE_RUNTIME_MODEL_CALLS_PER_SESSION - state.callCount),
        tokensRemaining: Math.max(
            0,
            MAX_PORTABLE_RUNTIME_MODEL_SESSION_TOKENS - state.chargedTokens,
        ),
        blockedByUnknownOutcome: state.blockedByUnknownOutcome,
    };
}

export function resetPortableRuntimeModelBudgetsForTests(): void {
    sessionBudgets.clear();
}
