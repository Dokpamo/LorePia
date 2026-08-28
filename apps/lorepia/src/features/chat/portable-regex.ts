import type { PortableRegexRequest, PortableRegexWorkerResult } from './portable-regex-operation';

export type PortableRegexResult = PortableRegexWorkerResult | { ok: false; timedOut: true };

type PortableRegexWorkerFactory = () => Worker;

const DEFAULT_TIMEOUT_MS = 75;
const MAX_CONCURRENT_WORKERS = 4;
const MAX_QUEUED_OPERATIONS = 256;
let workerFactoryOverride: PortableRegexWorkerFactory | null = null;
let activeWorkers = 0;
const workerWaiters: ((acquired: boolean) => void)[] = [];

export async function runPortableRegex(
    request: PortableRegexRequest,
    timeoutMs = DEFAULT_TIMEOUT_MS,
): Promise<PortableRegexResult> {
    if (!(await acquireWorkerSlot())) return { ok: false };
    try {
        return await runPortableRegexWorker(request, timeoutMs);
    } finally {
        releaseWorkerSlot();
    }
}

async function runPortableRegexWorker(
    request: PortableRegexRequest,
    timeoutMs: number,
): Promise<PortableRegexResult> {
    let worker: Worker;
    try {
        worker = (workerFactoryOverride ?? createPortableRegexWorker)();
    } catch {
        return { ok: false };
    }
    const id = globalThis.crypto.randomUUID();
    return await new Promise<PortableRegexResult>((resolve) => {
        let settled = false;
        const finish = (result: PortableRegexResult): void => {
            if (settled) return;
            settled = true;
            globalThis.clearTimeout(timeout);
            worker.removeEventListener('message', onMessage);
            worker.removeEventListener('error', onError);
            worker.terminate();
            resolve(result);
        };
        const onMessage = (event: MessageEvent<unknown>): void => {
            const response = event.data as {
                id?: unknown;
                result?: PortableRegexWorkerResult;
            };
            if (response.id !== id || response.result === undefined) return;
            finish(response.result);
        };
        const onError = (): void => finish({ ok: false });
        const timeout = globalThis.setTimeout(
            () => finish({ ok: false, timedOut: true }),
            Math.min(1_000, Math.max(1, timeoutMs)),
        );
        worker.addEventListener('message', onMessage);
        worker.addEventListener('error', onError);
        try {
            worker.postMessage({ id, request });
        } catch {
            finish({ ok: false });
        }
    });
}

async function acquireWorkerSlot(): Promise<boolean> {
    if (activeWorkers < MAX_CONCURRENT_WORKERS) {
        activeWorkers += 1;
        return true;
    }
    if (workerWaiters.length >= MAX_QUEUED_OPERATIONS) return false;
    return await new Promise<boolean>((resolve) => workerWaiters.push(resolve));
}

function releaseWorkerSlot(): void {
    const next = workerWaiters.shift();
    if (next === undefined) {
        activeWorkers -= 1;
    } else {
        next(true);
    }
}

export function setPortableRegexWorkerFactoryForTests(
    factory: PortableRegexWorkerFactory,
): () => void {
    const previous = workerFactoryOverride;
    workerFactoryOverride = factory;
    return () => {
        workerFactoryOverride = previous;
    };
}

function createPortableRegexWorker(): Worker {
    return new Worker(new URL('./portable-regex.worker.ts', import.meta.url), {
        type: 'module',
        name: 'lorepia-portable-regex',
    });
}
