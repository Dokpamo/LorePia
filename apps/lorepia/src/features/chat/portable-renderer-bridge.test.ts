import { afterEach, describe, expect, it, vi } from 'vitest';
import bridgeSource from './portable-renderer-bridge.js?raw';

function mountBridge(surface: 'room' | 'message') {
    const frames: (() => void)[] = [];
    const listeners = new Map<string, () => void>();
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
            createRange: () => document.createRange(),
            body: document.body,
            querySelector: (selector: string) => document.querySelector(selector),
            querySelectorAll: (selector: string) => document.querySelectorAll(selector),
            addEventListener: vi.fn(),
        },
        parent: { postMessage: publish },
        Node,
        HTMLElement,
        Element,
        HTMLMediaElement,
        HTMLButtonElement,
        HTMLInputElement,
        getComputedStyle: style,
        innerWidth: 800,
        innerHeight: 600,
        requestAnimationFrame: (callback: () => void) => frames.push(callback),
        addEventListener: (type: string, callback: () => void) => listeners.set(type, callback),
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
    const report = () => {
        listeners.get('resize')?.();
        const frame = frames.shift();
        if (!frame) throw new Error('resize layout was not scheduled');
        frame();
    };
    return { report, publish, style, frames };
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
        const section = document.querySelector('section');
        expect(section?.style.height).toBe('auto');
        expect(section?.style.overflowY).toBe('visible');
        expect(publish.mock.calls[0]?.[0]).toMatchObject({ type: 'portable_resize', height: 32 });
        report();
        expect(publish).toHaveBeenCalledOnce();
    });

    it('reports card hit regions before a clipped frame receives animation frames', () => {
        document.body.innerHTML =
            '<div class="portable-message"><button style="position:fixed">Card</button></div>';
        const button = document.querySelector('button');
        if (!button) throw new Error('Missing card control');
        button.getBoundingClientRect = () => new DOMRect(320, 12, 48, 48);
        const { publish, frames } = mountBridge('room');
        expect(frames).toHaveLength(0);
        expect(publish).toHaveBeenCalledWith(
            expect.objectContaining({
                type: 'portable_regions',
                regions: [{ x: 320, y: 12, width: 48, height: 48 }],
            }),
            '*',
        );
    });

    it('expands an imported scroll box before reporting its full initial height', () => {
        document.body.innerHTML =
            '<div class="portable-message"><section style="height:160px;max-height:160px;overflow-y:auto">Long message</section></div>';
        const root = document.querySelector<HTMLElement>('.portable-message');
        const section = document.querySelector('section');
        if (!root || !section) throw new Error('Missing message');
        root.getBoundingClientRect = () => new DOMRect(0, 0, 300, 3436);
        const { publish, frames } = mountBridge('message');
        expect(frames).toHaveLength(0);
        expect(publish).toHaveBeenCalledWith(
            expect.objectContaining({ type: 'portable_resize', height: 3436 }),
            '*',
        );
        expect(section.style.height).toBe('auto');
        expect(section.style.maxHeight).toBe('none');
        expect(section.style.overflowY).toBe('visible');
    });
});

function textRange(rectangles: DOMRect[]) {
    const selectNodeContents = vi.fn();
    const create = vi.spyOn(document, 'createRange').mockReturnValue({
        selectNodeContents,
        getClientRects: () => rectangles,
    } as unknown as Range);
    return { create, selectNodeContents };
}

it('reports direct card text line bounds synchronously and refreshes root visibility', () => {
    document.body.innerHTML = '<div class="portable-message">Plain card text</div>';
    const root = document.querySelector<HTMLElement>('.portable-message');
    if (!root) throw new Error('Missing text card');
    const { selectNodeContents } = textRange([
        new DOMRect(12, 20, 85, 18),
        new DOMRect(12, 38, 40, 18),
    ]);
    const { publish, frames, style, report } = mountBridge('room');
    expect(frames).toHaveLength(0);
    expect(selectNodeContents).toHaveBeenCalledWith(root.firstChild);
    expect(publish.mock.calls[0]?.[0]).toMatchObject({
        type: 'portable_regions',
        regions: [
            { x: 12, y: 20, width: 85, height: 18 },
            { x: 12, y: 38, width: 40, height: 18 },
        ],
    });
    expect(style).toHaveBeenCalledOnce();
    root.style.visibility = 'hidden';
    report();
    expect(publish.mock.calls[1]?.[0]).toMatchObject({ type: 'portable_regions', regions: [] });
    expect(style).toHaveBeenCalledTimes(2);
});

it.each(['display:none', 'visibility:hidden', 'visibility:collapse', 'opacity:0'])(
    'ignores hidden direct text with %s',
    (hidden) => {
        document.body.innerHTML = `<div class="portable-message" style="${hidden}">Hidden</div>`;
        const { create } = textRange([new DOMRect(0, 0, 100, 20)]);
        const { publish } = mountBridge('room');
        expect(create).not.toHaveBeenCalled();
        expect(publish.mock.calls[0]?.[0]).toMatchObject({ type: 'portable_regions', regions: [] });
    },
);

it('does not create text hit regions for whitespace-only root children', () => {
    document.body.innerHTML = '<div class="portable-message"> \n\t </div>';
    const { create } = textRange([new DOMRect(0, 0, 800, 600)]);
    const { publish } = mountBridge('room');
    expect(create).not.toHaveBeenCalled();
    expect(publish.mock.calls[0]?.[0]).toMatchObject({ type: 'portable_regions', regions: [] });
});

it('clips oversized text bounds and keeps the shared 128-region cap', () => {
    document.body.innerHTML =
        '<div class="portable-message"><button>Control</button>Many lines</div>';
    const button = document.querySelector('button');
    if (!button) throw new Error('Missing card control');
    button.getBoundingClientRect = () => new DOMRect(10, 10, 20, 20);
    textRange([
        new DOMRect(-20, -30, 1000, 900),
        new DOMRect(-20, -20, 1, 1),
        ...Array.from({ length: 200 }, () => new DOMRect(2, 3, 4, 5)),
    ]);
    const { publish } = mountBridge('room');
    const report = publish.mock.calls[0]?.[0] as { regions: unknown[] };
    expect(report.regions).toHaveLength(128);
    expect(report.regions[0]).toEqual({ x: 10, y: 10, width: 20, height: 20 });
    expect(report.regions[1]).toEqual({ x: 0, y: 0, width: 800, height: 600 });
    expect(report.regions[127]).toEqual({ x: 2, y: 3, width: 4, height: 5 });
});

it('bounds text line scanning even when every measured line is offscreen', () => {
    document.body.innerHTML = '<div class="portable-message">Offscreen lines</div>';
    textRange([
        ...Array.from({ length: 4096 }, () => new DOMRect(-10, -10, 1, 1)),
        new DOMRect(0, 0, 10, 10),
    ]);
    const { publish } = mountBridge('room');
    expect(publish.mock.calls[0]?.[0]).toMatchObject({ type: 'portable_regions', regions: [] });
});
