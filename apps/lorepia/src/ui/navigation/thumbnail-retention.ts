interface RetainedThumbnail {
    bytes: number;
    evict: () => void;
}

// Retain already decoded DOM media, never a descriptor or a renderer approval.
// These bounds cover offscreen images; entering the viewport removes the entry.
const MAX_RETAINED_BYTES = 64 * 1024 * 1024;
const MAX_RETAINED_IMAGES = 64;
const recent = new Map<HTMLElement, RetainedThumbnail>();
let retainedBytes = 0;

export function releaseThumbnail(element: HTMLElement): void {
    const entry = recent.get(element);
    if (!entry) return;
    retainedBytes -= entry.bytes;
    recent.delete(element);
}

export function retainThumbnail(element: HTMLElement, bytes: number, evict: () => void): void {
    releaseThumbnail(element);
    if (!Number.isSafeInteger(bytes) || bytes <= 0 || bytes > MAX_RETAINED_BYTES) {
        evict();
        return;
    }
    recent.set(element, { bytes, evict });
    retainedBytes += bytes;
    while (retainedBytes > MAX_RETAINED_BYTES || recent.size > MAX_RETAINED_IMAGES) {
        const oldest = recent.entries().next().value;
        if (!oldest) break;
        releaseThumbnail(oldest[0]);
        oldest[1].evict();
    }
}
