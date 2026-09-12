import { afterEach, expect, it, vi } from 'vitest';
import { observePortableMediaVisibility } from './portable-media-visibility';
afterEach(() => {
    document.body.replaceChildren();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});
it('reports viewport/document visibility only to its frame and disconnects after unmount', () => {
    let callback: IntersectionObserverCallback = vi.fn();
    const disconnect = vi.fn();
    vi.stubGlobal(
        'IntersectionObserver',
        class {
            constructor(value: IntersectionObserverCallback) {
                callback = value;
            }
            observe = vi.fn();
            disconnect = disconnect;
        },
    );
    const frame = document.createElement('iframe');
    document.body.append(frame);
    if (!frame.contentWindow) throw new Error('Missing window');
    const post = vi.spyOn(frame.contentWindow, 'postMessage').mockImplementation(() => undefined);
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    const stop = observePortableMediaVisibility(frame, () => 'runtime');
    expect(post.mock.calls.at(-1)?.[0]).toMatchObject({ visible: false, runtimeId: 'runtime' });
    callback([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver);
    expect(post.mock.calls.at(-1)?.[0]).toMatchObject({ visible: true });
    visibility.mockReturnValue('hidden');
    document.dispatchEvent(new Event('visibilitychange'));
    expect(post.mock.calls.at(-1)?.[0]).toMatchObject({ visible: false });
    const calls = post.mock.calls.length;
    stop();
    callback([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver);
    frame.dispatchEvent(new Event('load'));
    document.dispatchEvent(new Event('visibilitychange'));
    expect(post).toHaveBeenCalledTimes(calls);
    expect(disconnect).toHaveBeenCalledOnce();
});
