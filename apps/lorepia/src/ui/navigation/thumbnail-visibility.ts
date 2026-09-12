import { releaseThumbnail } from './thumbnail-retention';

type Visibility = (near: boolean) => void;
interface ThumbnailViewport {
    observer: IntersectionObserver;
    targets: Map<Element, Visibility>;
    dispose: () => void;
}
const viewports = new Map<Element | null, ThumbnailViewport>();

function thumbnailRoot(element: HTMLElement) {
    return element.closest('.ui-overlay-body, .seed-image-strip, .seed-scroll');
}

function distanceFromViewport(rect: DOMRectReadOnly, bounds?: DOMRectReadOnly): number {
    return Math.max(
        (bounds?.top ?? 0) - rect.bottom,
        rect.top - (bounds?.bottom ?? window.innerHeight),
        (bounds?.left ?? 0) - rect.right,
        rect.left - (bounds?.right ?? window.innerWidth),
        0,
    );
}

/** Re-evaluated when an IPC slot opens, so a scroll can overtake queued preloads. */
export function thumbnailDistance(element: HTMLElement): number {
    if (!element.isConnected || element.closest('[inert], [aria-hidden="true"]')) return Infinity;
    return distanceFromViewport(
        element.getBoundingClientRect(),
        thumbnailRoot(element)?.getBoundingClientRect(),
    );
}

function preparationMargin(root: Element | null): string {
    if (root?.matches('.seed-image-strip'))
        return `0px ${String((root.clientWidth || window.innerWidth) * 2)}px`;
    const height = root?.clientHeight ?? 0;
    return `${String((height > 0 ? height : window.innerHeight) * 2)}px 0px`;
}

function createViewport(root: Element | null): ThumbnailViewport {
    const targets = new Map<Element, Visibility>();
    const notify = (entries: IntersectionObserverEntry[]) => {
        // Observer delivery is asynchronous. Protect all entering images
        // before any departing image can evict a newly visible cached one.
        const latest = [...new Map(entries.map((entry) => [entry.target, entry])).values()].filter(
            (entry) => targets.has(entry.target),
        );
        const entering = latest.filter((entry) => entry.isIntersecting);
        for (const entry of entering) releaseThumbnail(entry.target as HTMLElement);
        // Request currently visible rows before the nearby rows.
        const bounds = entering.length > 1 ? root?.getBoundingClientRect() : undefined;
        entering.sort(
            (a, b) =>
                distanceFromViewport(a.boundingClientRect, bounds) -
                distanceFromViewport(b.boundingClientRect, bounds),
        );
        for (const entry of entering) targets.get(entry.target)?.(true);
        for (const entry of latest) if (!entry.isIntersecting) targets.get(entry.target)?.(false);
    };
    let rootMargin = preparationMargin(root);
    let generation = 0;
    function observer() {
        const epoch = ++generation;
        return new IntersectionObserver(
            (entries) => {
                if (epoch === generation) notify(entries);
            },
            { root, rootMargin },
        );
    }
    const viewport: ThumbnailViewport = {
        observer: observer(),
        targets,
        dispose() {
            generation++;
            viewport.observer.disconnect();
            resizeObserver?.disconnect();
            window.removeEventListener('resize', resize);
        },
    };
    function resize() {
        const next = preparationMargin(root);
        if (next === rootMargin) return;
        rootMargin = next;
        viewport.observer.disconnect();
        viewport.observer = observer();
        for (const target of targets.keys()) viewport.observer.observe(target);
    }
    const resizeObserver =
        typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(resize);
    if (root) resizeObserver?.observe(root);
    window.addEventListener('resize', resize);
    return viewport;
}

/** Prepare two screens ahead and behind, within the actual scrolling container. */
export function observeThumbnail(element: HTMLElement, update: Visibility): () => void {
    if (typeof IntersectionObserver === 'undefined') {
        update(true);
        return () => undefined;
    }
    const root = thumbnailRoot(element);
    let viewport = viewports.get(root);
    if (!viewport) {
        viewport = createViewport(root);
        viewports.set(root, viewport);
    }
    const current = viewport;
    current.targets.set(element, update);
    current.observer.observe(element);
    return () => {
        if (!current.targets.delete(element)) return;
        current.observer.unobserve(element);
        if (current.targets.size === 0) {
            current.dispose();
            viewports.delete(root);
        }
    };
}
