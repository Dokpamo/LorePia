import type {
    AssetDeliveryDto,
    AssetDeliverySelector,
    LorepiaClient,
} from '../../lib/ipc/contracts';
import { normalizeClientError } from '../../lib/ipc/errors';
import type { AssetLoadPriority } from './asset-load-priority';

const pending = new Map<() => void, AssetLoadPriority>();
let active = 0;
let backgroundDrainQueued = false;
const MAX_ACTIVE = 4;
const LOAD_TIMEOUT_MS = 30_000;

function drain() {
    while (active < MAX_ACTIVE) {
        let next: (() => void) | undefined;
        let nearest = Infinity;
        for (const [start, priority] of pending) {
            const distance = priority();
            if (!next || distance < nearest) {
                next = start;
                nearest = distance;
            }
        }
        if (!next) return;
        pending.delete(next);
        next();
    }
}

function scheduleBackgroundDrain() {
    if (backgroundDrainQueued) return;
    backgroundDrainQueued = true;
    // Collect the rest of this render's requests before background work takes
    // the first slots. Foreground requests still start synchronously.
    queueMicrotask(() => {
        backgroundDrainQueued = false;
        drain();
    });
}

function timeoutError() {
    return new DOMException('Asset verification timed out', 'TimeoutError');
}

function pause(delay: number, signal: AbortSignal): Promise<boolean> {
    return new Promise((resolve, reject) => {
        const page = typeof document === 'undefined' ? undefined : document;
        const view = page?.defaultView;
        let done = false;
        const cleanup = () => {
            clearTimeout(timer);
            signal.removeEventListener('abort', cancel);
            page?.removeEventListener('visibilitychange', resume);
            view?.removeEventListener('focus', resume);
            view?.removeEventListener('pageshow', resume);
        };
        const finish = (resumed: boolean) => {
            if (done) return;
            done = true;
            cleanup();
            resolve(resumed);
        };
        const resume = () => {
            if (page?.visibilityState !== 'hidden') finish(true);
        };
        const timer = setTimeout(() => finish(false), delay);
        const cancel = () => {
            if (done) return;
            done = true;
            cleanup();
            reject(new DOMException('Asset load cancelled', 'AbortError'));
        };
        signal.addEventListener('abort', cancel, { once: true });
        page?.addEventListener('visibilitychange', resume);
        view?.addEventListener('focus', resume);
        view?.addEventListener('pageshow', resume);
        if (signal.aborted) cancel();
    });
}

/** Retry waits never occupy an IPC slot or prevent another visible image loading. */
export async function loadAssetDelivery(
    client: Pick<LorepiaClient, 'resolveAssetDelivery'>,
    selector: AssetDeliverySelector,
    signal: AbortSignal,
    onWaiting?: () => void,
    priority: AssetLoadPriority = () => 0,
): Promise<AssetDeliveryDto> {
    let deadline = Date.now() + LOAD_TIMEOUT_MS;
    for (let attempt = 0; ; attempt++) {
        signal.throwIfAborted();
        const remaining = deadline - Date.now();
        if (remaining <= 0) throw timeoutError();
        try {
            const result = await enqueueAssetDelivery(
                client,
                selector,
                signal,
                remaining,
                priority,
            );
            signal.throwIfAborted();
            return result;
        } catch (error: unknown) {
            signal.throwIfAborted();
            const normalized = normalizeClientError(error);
            if (normalized.code !== 'storage_unavailable' || !normalized.recoverable) throw error;
            const remaining = deadline - Date.now();
            if (remaining <= 0) throw timeoutError();
            onWaiting?.();
            const delay = Math.min(250 * 2 ** Math.min(attempt, 3), remaining);
            if (await pause(delay, signal)) {
                // Returning to the app starts a fresh attempt even if background
                // WebView timers were suspended beyond the previous deadline.
                deadline = Date.now() + LOAD_TIMEOUT_MS;
            }
        }
    }
}

/** Presentation work only: never cache approval, change a URL, or skip verification. */
function enqueueAssetDelivery(
    client: Pick<LorepiaClient, 'resolveAssetDelivery'>,
    selector: AssetDeliverySelector,
    signal: AbortSignal,
    timeoutMs: number,
    priority: AssetLoadPriority,
): Promise<AssetDeliveryDto> {
    return new Promise((resolve, reject) => {
        let completed = false;
        function complete(deliver: () => void) {
            if (completed) return;
            completed = true;
            clearTimeout(timer);
            pending.delete(start);
            signal.removeEventListener('abort', cancel);
            deliver();
        }
        function cancel() {
            complete(() => reject(new DOMException('Asset load cancelled', 'AbortError')));
        }
        function start() {
            active++;
            void (async () => client.resolveAssetDelivery({ selector }))()
                .then(
                    (result) => complete(() => resolve(result)),
                    (error: unknown) =>
                        complete(() =>
                            reject(error instanceof Error ? error : normalizeClientError(error)),
                        ),
                )
                .finally(() => {
                    // A timed-out/cancelled consumer cannot release a native
                    // concurrency slot before the underlying call actually ends.
                    active--;
                    drain();
                });
        }
        const timer = setTimeout(() => complete(() => reject(timeoutError())), timeoutMs);
        signal.addEventListener('abort', cancel, { once: true });
        if (signal.aborted) cancel();
        else {
            pending.set(start, priority);
            if (priority() <= 0) drain();
            else scheduleBackgroundDrain();
        }
    });
}
