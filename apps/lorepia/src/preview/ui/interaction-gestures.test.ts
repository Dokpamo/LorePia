import { afterEach, describe, expect, it, vi } from 'vitest';
import { cardDrag } from './card-drag';
import { edgeBack, requestBack } from './edge-back';

const disposals: (() => void)[] = [];
afterEach(() => {
    disposals.splice(0).forEach((dispose) => dispose());
    document.body.replaceChildren();
    vi.useRealTimers();
    vi.restoreAllMocks();
});
function pointer(
    target: EventTarget,
    type: string,
    x: number,
    y: number,
    time: number,
    device = 'mouse',
    id = 1,
) {
    const event = new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        clientX: x,
        clientY: y,
        button: 0,
    });
    for (const [key, value] of Object.entries({
        pointerId: id,
        pointerType: device,
        isPrimary: id === 1,
        timeStamp: time,
    }))
        Object.defineProperty(event, key, { value });
    target.dispatchEvent(event);
    return event;
}
function captures(node: HTMLElement) {
    const ids = new Set<number>();
    node.setPointerCapture = (id) => {
        ids.add(id);
    };
    node.hasPointerCapture = (id) => ids.has(id);
    node.releasePointerCapture = (id) => {
        ids.delete(id);
    };
}
function rail() {
    const root = document.createElement('div');
    root.className = 'ui-preview';
    const node = document.createElement('div');
    const a = document.createElement('button');
    a.dataset.railId = 'a';
    const b = document.createElement('button');
    b.dataset.railId = 'b';
    node.append(a, b);
    root.append(node);
    document.body.append(root);
    root.getBoundingClientRect = () => new DOMRect(0, 0, 360, 700);
    node.getBoundingClientRect = () => new DOMRect(0, 60, 56, 300);
    a.getBoundingClientRect = () => new DOMRect(4, 68, 48, 48);
    b.getBoundingClientRect = () => new DOMRect(4, 124, 48, 48);
    Object.defineProperty(root, 'clientWidth', { value: 360 });
    captures(node);
    const ondrop = vi.fn();
    const action = cardDrag(node, { ondrop });
    disposals.push(() => action.destroy());
    return { root, node, a, ondrop, action };
}
function back(withUnderlay = false, beforeback?: () => boolean) {
    const node = document.createElement('div');
    const underlay = document.createElement('div');
    if (withUnderlay) {
        underlay.className = 'ui-track';
        underlay.style.translate = '3px 0px';
        const layer = document.createElement('div');
        layer.className = 'ui-editor-layer';
        layer.append(node);
        document.body.append(underlay, layer);
    } else document.body.append(node);
    node.getBoundingClientRect = () => new DOMRect(0, 0, 360, 700);
    Object.defineProperty(node, 'clientWidth', { value: 360 });
    captures(node);
    const onback = vi.fn();
    vi.spyOn(node, 'animate').mockImplementation(
        () => ({ finished: Promise.resolve(), cancel: vi.fn() }) as unknown as Animation,
    );
    const action = edgeBack(node, { onback, beforeback });
    disposals.push(() => action.destroy());
    return { node, onback, underlay };
}

describe('card organization gestures', () => {
    it('drags onto another tile without activating the release click', () => {
        const { root, node, a, ondrop } = rail();
        pointer(a, 'pointerdown', 24, 90, 0);
        pointer(window, 'pointermove', 24, 148, 100);
        expect(root.querySelector('.ui-card-drag-ghost')).not.toBeNull();
        pointer(window, 'pointerup', 24, 148, 180);
        expect(ondrop).toHaveBeenCalledExactlyOnceWith('a', 'b');
        const click = new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 });
        a.dispatchEvent(click);
        expect(click.defaultPrevented).toBe(true);
        expect(root.querySelector('.ui-card-drag-ghost')).toBeNull();
        expect(node.hasPointerCapture(1)).toBe(false);
    });
    it('requires a touch hold, while a quick vertical gesture scrolls the rail', () => {
        vi.useFakeTimers();
        const { root, node, a, ondrop } = rail();
        pointer(a, 'pointerdown', 24, 90, 0, 'touch');
        pointer(window, 'pointermove', 24, 70, 50, 'touch');
        vi.advanceTimersByTime(400);
        expect(root.querySelector('.ui-card-drag-ghost')).toBeNull();
        expect(node.scrollTop).toBe(20);
        pointer(window, 'pointerup', 24, 70, 450, 'touch');
        expect(ondrop).not.toHaveBeenCalled();
        pointer(a, 'pointerdown', 24, 90, 500, 'touch');
        vi.advanceTimersByTime(300);
        pointer(window, 'pointermove', 24, 148, 850, 'touch');
        pointer(window, 'pointerup', 24, 148, 900, 'touch');
        expect(ondrop).toHaveBeenCalledExactlyOnceWith('a', 'b');
    });
    it('cancels on resize or unmount and removes ghost, capture and timer work', () => {
        vi.useFakeTimers();
        const { root, a, ondrop, action } = rail();
        pointer(a, 'pointerdown', 24, 90, 0);
        pointer(window, 'pointermove', 200, 200, 100);
        window.dispatchEvent(new Event('resize'));
        pointer(window, 'pointerup', 24, 148, 200);
        expect(ondrop).not.toHaveBeenCalled();
        pointer(a, 'pointerdown', 24, 90, 300, 'touch');
        action.destroy();
        vi.advanceTimersByTime(500);
        expect(root.querySelector('.ui-card-drag-ghost')).toBeNull();
        expect(vi.getTimerCount()).toBe(0);
    });
});

describe('edge back on settings and editors', () => {
    it('returns a completed swipe to the page when unsaved changes block dismissal', async () => {
        const beforeback = vi.fn(() => false);
        const { node, onback } = back(false, beforeback);
        pointer(node, 'pointerdown', 20, 200, 0, 'touch');
        pointer(window, 'pointermove', 240, 200, 200, 'touch');
        pointer(window, 'pointerup', 240, 200, 300, 'touch');
        await Promise.resolve();
        expect(beforeback).toHaveBeenCalledOnce();
        expect(onback).not.toHaveBeenCalled();
        expect(node).not.toHaveAttribute('data-back-dismissed');
        expect(node.style.getPropertyValue('--ui-back-offset')).toBe('');
    });
    it('reserves the editor edge before WebKit starts native text tracking', async () => {
        const { node, onback } = back();
        const input = document.createElement('textarea');
        node.append(input);
        const interior = pointer(input, 'pointerdown', 80, 200, 0);
        expect(interior.defaultPrevented).toBe(false);
        expect(node.hasPointerCapture(1)).toBe(false);
        const edge = pointer(input, 'pointerdown', 28, 200, 100);
        expect(edge.defaultPrevented).toBe(true);
        expect(node.hasPointerCapture(1)).toBe(true);
        pointer(window, 'pointermove', 240, 200, 300);
        pointer(window, 'pointerup', 240, 200, 400);
        await Promise.resolve();
        expect(onback).toHaveBeenCalledOnce();
        expect(node.hasPointerCapture(1)).toBe(false);
    });
    it('lets a new gesture interrupt a button-triggered departure at its visible position', async () => {
        const { node, onback } = back();
        let resolve: () => void = () => undefined;
        const cancelAnimation = vi.fn();
        vi.spyOn(node, 'animate').mockReturnValue({
            finished: new Promise<void>((done) => {
                resolve = done;
            }),
            cancel: cancelAnimation,
        } as unknown as Animation);
        requestBack(node);
        expect(onback).not.toHaveBeenCalled();
        const computed = vi
            .spyOn(window, 'getComputedStyle')
            .mockReturnValue({ transform: 'matrix(1, 0, 0, 1, 90, 0)' } as CSSStyleDeclaration);
        pointer(node, 'pointerdown', 100, 200, 0);
        computed.mockRestore();
        pointer(window, 'pointermove', 120, 200, 30);
        expect(node.style.getPropertyValue('--ui-back-offset')).toBe('110px');
        expect(cancelAnimation).toHaveBeenCalled();
        resolve();
        await Promise.resolve();
        expect(onback).not.toHaveBeenCalled();
        window.dispatchEvent(new Event('resize'));
        expect(node.style.getPropertyValue('--ui-back-offset')).toBe('');
    });
    it('moves the previous surface more slowly and cleans up on cancellation', () => {
        const { node, underlay, onback } = back(true);
        pointer(node, 'pointerdown', 8, 200, 0, 'touch');
        pointer(window, 'pointermove', 138, 200, 200, 'touch');
        expect(Number.parseFloat(underlay.style.translate)).toBeLessThan(0);
        expect(Math.abs(Number.parseFloat(underlay.style.translate))).toBeLessThan(130);
        window.dispatchEvent(new Event('resize'));
        expect(underlay.style.translate).toBe('3px 0px');
        expect(onback).not.toHaveBeenCalled();
    });
    it('follows the finger and returns after a committed edge gesture', async () => {
        const { node, onback } = back();
        pointer(node, 'pointerdown', 8, 200, 0, 'touch');
        pointer(window, 'pointermove', 138, 200, 200, 'touch');
        expect(node.style.getPropertyValue('--ui-back-offset')).toBe('130px');
        expect(onback).not.toHaveBeenCalled();
        pointer(window, 'pointerup', 138, 200, 300, 'touch');
        await Promise.resolve();
        expect(onback).toHaveBeenCalledOnce();
        expect(node.hasPointerCapture(1)).toBe(false);
    });
    it('restores a short drag and leaves interior editing and vertical scroll alone', async () => {
        const { node, onback } = back();
        pointer(node, 'pointerdown', 8, 200, 0);
        pointer(window, 'pointermove', 30, 200, 300);
        pointer(window, 'pointerup', 30, 200, 500);
        await Promise.resolve();
        expect(onback).not.toHaveBeenCalled();
        const input = document.createElement('textarea');
        node.append(input);
        pointer(input, 'pointerdown', 80, 200, 600);
        pointer(window, 'pointermove', 250, 200, 700);
        pointer(window, 'pointerup', 250, 200, 800);
        pointer(node, 'pointerdown', 8, 200, 900);
        const vertical = pointer(window, 'pointermove', 10, 250, 950);
        pointer(window, 'pointerup', 200, 250, 1000);
        expect(vertical.defaultPrevented).toBe(false);
        expect(onback).not.toHaveBeenCalled();
    });
    it('accepts a rightward drag from the interior of a settings surface', async () => {
        const { node, onback } = back();
        pointer(node, 'pointerdown', 70, 300, 0);
        pointer(window, 'pointermove', 245, 303, 220);
        pointer(window, 'pointerup', 245, 303, 330);
        await Promise.resolve();
        expect(onback).toHaveBeenCalledOnce();
    });
    it('accepts trackpad back and keeps vertical wheel scrolling native', async () => {
        vi.useFakeTimers();
        const { node, onback } = back();
        const vertical = new WheelEvent('wheel', { deltaY: 80, bubbles: true, cancelable: true });
        node.dispatchEvent(vertical);
        expect(vertical.defaultPrevented).toBe(false);
        vi.advanceTimersByTime(200);
        for (let i = 0; i < 8; i++) {
            node.dispatchEvent(
                new WheelEvent('wheel', { deltaX: -12, bubbles: true, cancelable: true }),
            );
            vi.advanceTimersByTime(16);
        }
        expect(onback).not.toHaveBeenCalled();
        await vi.advanceTimersByTimeAsync(200);
        expect(onback).toHaveBeenCalledOnce();
    });
    it('cancels a captured edge gesture on a second touch', () => {
        const { node, onback } = back();
        pointer(node, 'pointerdown', 8, 200, 0, 'touch');
        pointer(window, 'pointermove', 138, 200, 200, 'touch');
        pointer(node, 'pointerdown', 12, 200, 220, 'touch', 2);
        pointer(window, 'pointerup', 138, 200, 300, 'touch');
        expect(onback).not.toHaveBeenCalled();
        expect(node.hasPointerCapture(1)).toBe(false);
        expect(node.style.getPropertyValue('--ui-back-offset')).toBe('');
    });
});
