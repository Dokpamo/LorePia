import { afterEach, describe, expect, it, vi } from 'vitest';
import bridgeSource from './portable-renderer-bridge.js?raw';

afterEach(() => {
    document.body.replaceChildren();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

async function runBridge(surface: 'room' | 'message') {
    const { runInNewContext } = await vi.importActual<{
        runInNewContext: (source: string, globals: Record<string, unknown>) => void;
    }>('node:vm');
    const script = document.createElement('script');
    script.dataset.runtimeId = '12345678-1234-1234-1234-123456789abc';
    script.dataset.surface = surface;
    vi.spyOn(document, 'currentScript', 'get').mockReturnValue(script);
    const publish = vi.fn();
    // Chromium can suspend rAF for an opaque iframe with an empty clip path.
    // Its initial region measurement must not depend on a visible animation frame.
    runInNewContext(bridgeSource, {
        document,
        HTMLElement,
        HTMLButtonElement,
        HTMLInputElement,
        HTMLMediaElement,
        Element,
        getComputedStyle,
        parent: { postMessage: publish },
        innerWidth: 384,
        innerHeight: 832,
        requestAnimationFrame: vi.fn(),
        addEventListener: vi.fn(),
        MutationObserver: class {
            observe = vi.fn();
        },
    });
    return publish;
}

describe('portable frame layout bridge', () => {
    it('reports card hit regions before the clipped frame can receive animation frames', async () => {
        document.body.innerHTML =
            '<div class="portable-message"><button style="position:fixed">Card</button></div>';
        const button = document.querySelector('button');
        if (!button) throw new Error('Missing card control');
        button.getBoundingClientRect = () => new DOMRect(320, 12, 48, 48);
        expect(await runBridge('room')).toHaveBeenCalledWith(
            expect.objectContaining({
                type: 'portable_regions',
                regions: [{ x: 320, y: 12, width: 48, height: 48 }],
            }),
            '*',
        );
    });

    it('expands an imported scroll box into the transcript before reporting its height', async () => {
        document.body.innerHTML =
            '<div class="portable-message"><section style="height:160px;max-height:160px;overflow-y:auto">Long message</section></div>';
        const root = document.querySelector<HTMLElement>('.portable-message');
        const section = document.querySelector('section');
        if (!root || !section) throw new Error('Missing message');
        root.getBoundingClientRect = () => new DOMRect(0, 0, 300, 3436);
        expect(await runBridge('message')).toHaveBeenCalledWith(
            expect.objectContaining({ type: 'portable_resize', height: 3436 }),
            '*',
        );
        expect(section.style.height).toBe('auto');
        expect(section.style.maxHeight).toBe('none');
        expect(section.style.overflowY).toBe('visible');
    });
});
