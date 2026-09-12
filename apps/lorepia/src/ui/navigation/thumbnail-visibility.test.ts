import { afterEach, expect, it, vi } from 'vitest';
import { observeThumbnail } from './thumbnail-visibility';
import { releaseThumbnail, retainThumbnail } from './thumbnail-retention';

const stop: (() => void)[] = [];
const elements: HTMLElement[] = [];
afterEach(() => {
    stop.splice(0).forEach((dispose) => dispose());
    elements.splice(0).forEach((element) => {
        releaseThumbnail(element);
        element.remove();
    });
    vi.unstubAllGlobals();
});

function viewport() {
    const instances: Observer[] = [];
    class Observer {
        observe = vi.fn();
        unobserve = vi.fn();
        disconnect = vi.fn();
        constructor(
            readonly notify: IntersectionObserverCallback,
            readonly options: IntersectionObserverInit,
        ) {
            instances.push(this);
        }
        deliver(entries: { target: HTMLElement; near: boolean; top: number }[]) {
            this.notify(
                entries.map(({ target, near, top }) => ({
                    target,
                    isIntersecting: near,
                    boundingClientRect: new DOMRect(12, top, 100, 100),
                    intersectionRect: new DOMRect(12, top, 100, 100),
                    intersectionRatio: near ? 1 : 0,
                    rootBounds: null,
                    time: 0,
                })),
                this as unknown as IntersectionObserver,
            );
        }
    }
    vi.stubGlobal('IntersectionObserver', Observer);
    const root = document.createElement('div');
    root.className = 'ui-overlay-body';
    Object.defineProperty(root, 'clientHeight', { value: 700, configurable: true });
    root.getBoundingClientRect = () => new DOMRect(0, 50, 390, 700);
    document.body.append(root);
    elements.push(root);
    function tile(update: (near: boolean) => void) {
        const element = document.createElement('span');
        root.append(element);
        elements.push(element);
        stop.push(observeThumbnail(element, update));
        return element;
    }
    function observer() {
        const result = instances[0];
        if (!result) throw new Error('Expected an active thumbnail observer');
        return result;
    }
    return { root, instances, tile, observer };
}

it('prepares two screens beyond the clipped scroller while prioritizing images already in view', () => {
    const { root, instances, tile, observer } = viewport();
    const updates: string[] = [];
    const nearby = tile((near) => {
        if (near) updates.push('nearby');
    });
    const visible = tile((near) => {
        if (near) updates.push('visible');
    });
    const far = tile((near) => {
        if (near) updates.push('far');
    });
    // A document-root margin cannot extend a nested scroller's clipping edge.
    expect(instances).toHaveLength(1);
    expect(observer().options.root).toBe(root);
    expect(observer().options.rootMargin).toBe('1400px 0px');
    observer().deliver([
        { target: nearby, near: true, top: 1800 },
        { target: far, near: false, top: 2400 },
        { target: visible, near: true, top: 300 },
    ]);
    expect(updates).toEqual(['visible', 'nearby']);
});

it('protects every returning image before offscreen retention can evict it in the same frame', () => {
    const { observer, tile } = viewport();
    const bytes = 24 * 1024 * 1024;
    const evictReturning = vi.fn();
    const evictDeparting = vi.fn();
    const departing = tile((near) => {
        if (!near) retainThumbnail(departing, bytes, evictDeparting);
    });
    const returning = tile(vi.fn());
    const other = tile(vi.fn());
    retainThumbnail(returning, bytes, evictReturning);
    retainThumbnail(other, bytes, vi.fn());
    // Processing the departing tile first would exceed 64 MiB and evict the
    // oldest tile, even though that tile has already returned to the screen.
    observer().deliver([
        { target: departing, near: false, top: -500 },
        { target: returning, near: true, top: 300 },
    ]);
    expect(evictReturning).not.toHaveBeenCalled();
    expect(evictDeparting).not.toHaveBeenCalled();
});

it('resizes the prepared region without allowing the old observer to unload a warm image', () => {
    const { root, instances, observer, tile } = viewport();
    const update = vi.fn();
    const image = tile(update);
    const old = observer();
    Object.defineProperty(root, 'clientHeight', { value: 900 });
    window.dispatchEvent(new Event('resize'));
    const next = instances[1];
    if (!next) throw new Error('Expected a resized viewport observer');
    expect(next.options.rootMargin).toBe('1800px 0px');
    next.deliver([{ target: image, near: true, top: 1500 }]);
    old.deliver([{ target: image, near: false, top: 1500 }]);
    expect(update).toHaveBeenCalledExactlyOnceWith(true);
});

it('ignores stale deliveries after cleanup and releases the shared observer with its last tile', () => {
    const { observer, tile } = viewport();
    const update = vi.fn();
    const first = tile(update);
    tile(vi.fn());
    stop[0]?.();
    expect(observer().unobserve).toHaveBeenCalledExactlyOnceWith(first);
    expect(observer().disconnect).not.toHaveBeenCalled();
    observer().deliver([{ target: first, near: true, top: 300 }]);
    expect(update).not.toHaveBeenCalled();
    stop[1]?.();
    expect(observer().disconnect).toHaveBeenCalledOnce();
});

it('reads the shared viewport once for a large observer delivery', () => {
    const { root, observer, tile } = viewport();
    const measure = vi.spyOn(root, 'getBoundingClientRect');
    const updates: number[] = [];
    const tiles = Array.from({ length: 200 }, (_, index) =>
        tile((near) => {
            if (near) updates.push(index);
        }),
    );
    observer().deliver(
        tiles.map((target, index) => ({
            target,
            near: true,
            top: 1000 + (200 - index) * 100,
        })),
    );
    expect(measure).toHaveBeenCalledOnce();
    expect(updates).toEqual(Array.from({ length: 200 }, (_, index) => 199 - index));
});
