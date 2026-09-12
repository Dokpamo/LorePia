interface RetainedThumbnail {
    bytes: number;
    expiresAt: number;
    evict: () => void;
}

// Retain already decoded DOM media, never a descriptor or a renderer approval.
// These bounds cover offscreen images; entering the viewport removes the entry.
const MAX_RETAINED_BYTES = 64 * 1024 * 1024;
const MAX_RETAINED_IMAGES = 64;
const RETENTION_MS = 30_000;
const recent = new Map<HTMLElement, RetainedThumbnail>();
let retainedBytes = 0;
let expiryTimer: ReturnType<typeof setTimeout> | undefined;

function scheduleExpiry(): void {
    if (recent.size === 0) {
        clearTimeout(expiryTimer);
        expiryTimer = undefined;
        return;
    }
    if (expiryTimer !== undefined) return;
    const oldest = recent.values().next().value;
    if (!oldest) return;
    // One timer for the whole offscreen cache, with no work while it is empty.
    expiryTimer = setTimeout(
        () => {
            expiryTimer = undefined;
            const now = performance.now();
            for (const [element, entry] of recent) {
                if (entry.expiresAt > now) continue;
                retainedBytes -= entry.bytes;
                recent.delete(element);
                entry.evict();
            }
            scheduleExpiry();
        },
        Math.max(0, oldest.expiresAt - performance.now()),
    );
}

export function releaseThumbnail(element: HTMLElement): void {
    const entry = recent.get(element);
    if (!entry) return;
    retainedBytes -= entry.bytes;
    recent.delete(element);
    scheduleExpiry();
}

export function retainThumbnail(element: HTMLElement, bytes: number, evict: () => void): void {
    releaseThumbnail(element);
    if (!Number.isSafeInteger(bytes) || bytes <= 0 || bytes > MAX_RETAINED_BYTES) {
        evict();
        return;
    }
    recent.set(element, { bytes, expiresAt: performance.now() + RETENTION_MS, evict });
    retainedBytes += bytes;
    while (retainedBytes > MAX_RETAINED_BYTES || recent.size > MAX_RETAINED_IMAGES) {
        const oldest = recent.entries().next().value;
        if (!oldest) break;
        releaseThumbnail(oldest[0]);
        oldest[1].evict();
    }
    scheduleExpiry();
}
