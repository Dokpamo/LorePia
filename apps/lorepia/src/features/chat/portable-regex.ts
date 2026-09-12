import {
    isPortableRegexWorkerResponse,
    type PortableRegexRequest,
    type PortableRegexWorkerFailureReason,
    type PortableRegexWorkerResult,
} from './portable-regex-protocol';
import { renderPortableMacros } from './portable-display';

export type PortableRegexFailureReason =
    | PortableRegexWorkerFailureReason
    | 'disabled'
    | 'execution_timeout'
    | 'queue_timeout'
    | 'queue_full'
    | 'worker_error';

export type PortableRegexResult =
    | Extract<PortableRegexWorkerResult, { ok: true }>
    | {
          ok: false;
          reason: PortableRegexFailureReason;
          timedOut?: true;
          disabled?: true;
          disabledReason?: Exclude<PortableRegexFailureReason, 'disabled'>;
      };

export interface PortableRegexRunOptions {
    timeoutMs?: number;
    /** Stable card/revision/rule identity. Imported content cannot choose cache policy. */
    ruleKey?: string;
}

type PortableRegexWorkerFactory = () => Worker;

const DEFAULT_TIMEOUT_MS = 75;
const MAX_CONCURRENT_WORKERS = 4;
const MAX_QUEUED_OPERATIONS = 256;
const MAX_DISABLED_RULES = 512;
const WORKER_STARTUP_TIMEOUT_MS = 1_000;
const WORKER_IDLE_TIMEOUT_MS = 30_000;
const idleWorkers: { worker: Worker; timer: ReturnType<typeof globalThis.setTimeout> }[] = [];
let workerFactoryOverride: PortableRegexWorkerFactory | null = null;
let activeWorkers = 0;
interface WorkerWaiter {
    resolve: (result: 'acquired' | 'queue_timeout') => void;
    timer: ReturnType<typeof globalThis.setTimeout>;
}
const workerWaiters: WorkerWaiter[] = [];
const disabledRules = new Map<string, Exclude<PortableRegexFailureReason, 'disabled'>>();

export interface PortableRegexReviewRule {
    id: string;
    phase: string;
    runtime_index: number;
    pattern: string;
    flags: string;
}

export interface PortableRegexReviewResult {
    id: string;
    status: 'valid' | 'invalid' | 'timed_out' | 'unavailable';
}

export async function runPortableRegex(
    request: PortableRegexRequest,
    timeoutOrOptions: number | PortableRegexRunOptions = DEFAULT_TIMEOUT_MS,
): Promise<PortableRegexResult> {
    const options =
        typeof timeoutOrOptions === 'number' ? { timeoutMs: timeoutOrOptions } : timeoutOrOptions;
    const timeoutMs = boundedTimeout(options.timeoutMs ?? DEFAULT_TIMEOUT_MS);
    const cacheKey = portableRegexCacheKey(options.ruleKey, request.pattern, request.flags);
    if (cacheKey !== null) {
        const reason = disabledRules.get(cacheKey);
        if (reason !== undefined) {
            return {
                ok: false,
                reason: 'disabled',
                disabled: true,
                disabledReason: reason,
            };
        }
    }
    const deadline = Date.now() + timeoutMs;
    const slot = await acquireWorkerSlot(deadline);
    if (slot !== 'acquired') {
        return {
            ok: false,
            reason: slot,
            timedOut: true,
        };
    }
    try {
        const remainingMs = deadline - Date.now();
        if (remainingMs <= 0) {
            return { ok: false, reason: 'queue_timeout', timedOut: true };
        }
        const result = await runPortableRegexWorker(request, remainingMs);
        if (
            cacheKey !== null &&
            !result.ok &&
            (result.reason === 'execution_timeout' || result.reason === 'invalid_pattern')
        ) {
            disableRule(cacheKey, result.reason);
        }
        return result;
    } finally {
        releaseWorkerSlot();
    }
}

export async function inspectPortableRegexRules(
    rules: readonly PortableRegexReviewRule[],
    reviewScope: string,
): Promise<PortableRegexReviewResult[]> {
    const results = new Array<PortableRegexReviewResult>(rules.length);
    let cursor = 0;
    const runner = async (): Promise<void> => {
        while (cursor < rules.length) {
            const index = cursor;
            cursor += 1;
            const rule = rules[index];
            if (rule === undefined) continue;
            const result = await runPortableRegex(
                {
                    operation: 'compile',
                    pattern: rule.flags.includes('<cbs>')
                        ? renderPortableMacros(
                              rule.pattern,
                              { variables: {}, lastMessageId: -1, chatIndex: -1 },
                              '',
                          )
                        : rule.pattern,
                    flags: rule.flags,
                },
                {
                    timeoutMs: DEFAULT_TIMEOUT_MS,
                    ruleKey: portableRegexRuleKey(
                        reviewScope,
                        rule.phase,
                        rule.id,
                        rule.runtime_index,
                    ),
                },
            );
            results[index] = {
                id: rule.id,
                status: portableRegexReviewStatus(result),
            };
        }
    };
    await Promise.all(
        Array.from({ length: Math.min(MAX_CONCURRENT_WORKERS, rules.length) }, runner),
    );
    return results;
}

export function portableRegexRuleKey(
    scope: string,
    phase: string,
    ruleId: string,
    runtimeIndex: number,
): string {
    return JSON.stringify([scope, phase, ruleId, runtimeIndex]);
}

async function runPortableRegexWorker(
    request: PortableRegexRequest,
    timeoutMs: number,
): Promise<PortableRegexResult> {
    const cached = idleWorkers.pop();
    if (cached !== undefined) globalThis.clearTimeout(cached.timer);
    let worker: Worker;
    try {
        worker = cached?.worker ?? (workerFactoryOverride ?? createPortableRegexWorker)();
    } catch {
        return { ok: false, reason: 'worker_error' };
    }
    const id = globalThis.crypto.randomUUID();
    return await new Promise<PortableRegexResult>((resolve) => {
        let settled = false;
        let started = false;
        let timeout: ReturnType<typeof globalThis.setTimeout>;
        const finish = (result: PortableRegexResult, reusable = false): void => {
            if (settled) return;
            settled = true;
            globalThis.clearTimeout(timeout);
            worker.removeEventListener('message', onMessage);
            worker.removeEventListener('error', onError);
            if (reusable) retainIdleWorker(worker);
            else worker.terminate();
            resolve(result);
        };
        const start = (): void => {
            if (started || settled) return;
            started = true;
            globalThis.clearTimeout(timeout);
            // Preserve the execution budget. Worker boot is separately bounded and
            // cannot incorrectly disable a rule before that rule has even run.
            timeout = globalThis.setTimeout(
                () => finish({ ok: false, reason: 'execution_timeout', timedOut: true }),
                boundedTimeout(timeoutMs),
            );
            try {
                worker.postMessage({ id, request });
            } catch {
                finish({ ok: false, reason: 'worker_error' });
            }
        };
        const onMessage = (event: MessageEvent<unknown>): void => {
            if (event.data === 'portable_regex_ready') {
                start();
                return;
            }
            if (!started || !isPortableRegexWorkerResponse(event.data) || event.data.id !== id)
                return;
            finish(event.data.result, true);
        };
        const onError = (): void => finish({ ok: false, reason: 'worker_error' });
        timeout = globalThis.setTimeout(onError, WORKER_STARTUP_TIMEOUT_MS);
        worker.addEventListener('message', onMessage);
        worker.addEventListener('error', onError);
        if (cached !== undefined) start();
    });
}

function retainIdleWorker(worker: Worker): void {
    const entry = {
        worker,
        timer: globalThis.setTimeout(() => {
            const index = idleWorkers.indexOf(entry);
            if (index >= 0) idleWorkers.splice(index, 1);
            worker.terminate();
        }, WORKER_IDLE_TIMEOUT_MS),
    };
    idleWorkers.push(entry);
}

function clearIdleWorkers(): void {
    for (const { worker, timer } of idleWorkers.splice(0)) {
        globalThis.clearTimeout(timer);
        worker.terminate();
    }
}

async function acquireWorkerSlot(
    deadline: number,
): Promise<'acquired' | 'queue_timeout' | 'queue_full'> {
    if (activeWorkers < MAX_CONCURRENT_WORKERS) {
        activeWorkers += 1;
        return 'acquired';
    }
    if (workerWaiters.length >= MAX_QUEUED_OPERATIONS) return 'queue_full';
    const remainingMs = deadline - Date.now();
    if (remainingMs <= 0) return 'queue_timeout';
    return await new Promise<'acquired' | 'queue_timeout'>((resolve) => {
        const waiter: WorkerWaiter = {
            resolve,
            timer: globalThis.setTimeout(() => {
                const index = workerWaiters.indexOf(waiter);
                if (index >= 0) workerWaiters.splice(index, 1);
                resolve('queue_timeout');
            }, remainingMs),
        };
        workerWaiters.push(waiter);
    });
}

function releaseWorkerSlot(): void {
    const next = workerWaiters.shift();
    if (next === undefined) {
        activeWorkers = Math.max(0, activeWorkers - 1);
    } else {
        globalThis.clearTimeout(next.timer);
        next.resolve('acquired');
    }
}

function boundedTimeout(value: number): number {
    return Math.min(1_000, Math.max(1, Number.isFinite(value) ? value : DEFAULT_TIMEOUT_MS));
}

function portableRegexCacheKey(
    ruleKey: string | undefined,
    pattern: string,
    flags: string,
): string | null {
    const key = ruleKey?.trim();
    if (key === undefined || key === '' || key.length > 8_192) return null;
    return `${key}\0${flags}\0${pattern}`;
}

function disableRule(
    cacheKey: string,
    reason: Exclude<PortableRegexFailureReason, 'disabled'>,
): void {
    disabledRules.delete(cacheKey);
    disabledRules.set(cacheKey, reason);
    while (disabledRules.size > MAX_DISABLED_RULES) {
        const oldest = disabledRules.keys().next().value;
        if (oldest === undefined) break;
        disabledRules.delete(oldest);
    }
}

export function resetPortableRegexRuleFailuresForTests(): void {
    disabledRules.clear();
}

function portableRegexReviewStatus(
    result: PortableRegexResult,
): PortableRegexReviewResult['status'] {
    if (result.ok) return 'valid';
    const reason = result.reason === 'disabled' ? result.disabledReason : result.reason;
    if (reason === 'invalid_pattern') return 'invalid';
    if (reason === 'execution_timeout') return 'timed_out';
    return 'unavailable';
}

export function setPortableRegexWorkerFactoryForTests(
    factory: PortableRegexWorkerFactory,
): () => void {
    clearIdleWorkers();
    const previous = workerFactoryOverride;
    workerFactoryOverride = factory;
    return () => {
        clearIdleWorkers();
        workerFactoryOverride = previous;
    };
}

function createPortableRegexWorker(): Worker {
    return new Worker(new URL('./portable-regex.worker.ts', import.meta.url), {
        type: 'module',
        name: 'lorepia-portable-regex',
    });
}
