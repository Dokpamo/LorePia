import { afterEach, describe, expect, it, vi } from 'vitest';
import bridgeSource from './portable-renderer-bridge.js?raw';

function mountBridge(surface: 'room' | 'message') {
    const frames: (() => void)[] = [];
    const publish = vi.fn();
    const style = vi.fn((element: Element) => getComputedStyle(element));
    const script = document.createElement('script');
    script.dataset.runtimeId = '00000000-0000-0000-0000-000000000000';
    script.dataset.surface = surface;
    vi.spyOn(document, 'currentScript', 'get').mockReturnValue(script);
    // Execute the bundled trusted bridge with isolated globals; no listeners survive the test.
    const context = {
        document: {
            currentScript: script,
            body: document.body,
            querySelector: (selector: string) => document.querySelector(selector),
            querySelectorAll: (selector: string) => document.querySelectorAll(selector),
            addEventListener: vi.fn(),
        },
        parent: { postMessage: publish },
        HTMLElement,
        Element,
        HTMLMediaElement,
        HTMLButtonElement,
        HTMLInputElement,
        getComputedStyle: style,
        innerWidth: 800,
        innerHeight: 600,
        requestAnimationFrame: (callback: () => void) => frames.push(callback),
        addEventListener: vi.fn(),
        ResizeObserver: undefined,
        MutationObserver: class {
            observe() {
                // Layout is advanced explicitly by this test.
            }
        },
    };
    // The script is the repository's trusted bridge, never imported character markup.
    // eslint-disable-next-line @typescript-eslint/no-implied-eval
    const execute = new Function(...Object.keys(context), 'globalThis', bridgeSource) as (
        ...values: unknown[]
    ) => void;
    execute(...Object.values(context), context);
    const report = frames[0];
    if (!report) throw new Error('initial layout was not scheduled');
    return { report, publish, style };
}

afterEach(() => {
    document.body.replaceChildren();
    vi.restoreAllMocks();
});

describe('portable renderer bridge layout', () => {
    it('reads room styles once per element, then refreshes visibility on the next report', () => {
        document.body.innerHTML =
            '<div class="portable-message"><section style="opacity:1"><div><button style="position:fixed">A</button><button style="position:fixed">B</button></div></section></div>';
        vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
            x: 10,
            y: 20,
            left: 10,
            top: 20,
            right: 110,
            bottom: 70,
            width: 100,
            height: 50,
            toJSON: () => ({}),
        });
        const { report, publish, style } = mountBridge('room');
        report();
        expect(style).toHaveBeenCalledTimes(4);
        expect(publish.mock.calls[0]?.[0]).toMatchObject({
            type: 'portable_regions',
            regions: Array.from({ length: 3 }, () => ({ x: 10, y: 20, width: 100, height: 50 })),
        });
        const section = document.querySelector('section');
        if (!section) throw new Error('section missing');
        section.style.opacity = '0';
        report();
        expect(style).toHaveBeenCalledTimes(8);
        expect(publish.mock.calls[1]?.[0]).toMatchObject({ type: 'portable_regions', regions: [] });
        report();
        expect(publish).toHaveBeenCalledTimes(2);
    });

    it('preserves message overflow normalization and resize deduplication', () => {
        document.body.innerHTML =
            '<div class="portable-message"><section style="overflow-y:scroll;height:100px">message</section></div>';
        const { report, publish } = mountBridge('message');
        report();
        const section = document.querySelector('section');
        expect(section?.style.height).toBe('auto');
        expect(section?.style.overflowY).toBe('visible');
        expect(publish.mock.calls[0]?.[0]).toMatchObject({ type: 'portable_resize', height: 32 });
        report();
        expect(publish).toHaveBeenCalledOnce();
    });
});
