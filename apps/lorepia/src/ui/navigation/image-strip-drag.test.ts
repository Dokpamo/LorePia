import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { imageStripDrag } from './image-strip-drag';

const disposals: (() => void)[] = [];
let frames: Map<number, FrameRequestCallback>;
beforeEach(() => {
    frames = new Map();
    let id = 0;
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
        frames.set(++id, callback);
        return id;
    });
    vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
    vi.spyOn(performance, 'now').mockReturnValue(0);
});
afterEach(() => {
    disposals.splice(0).forEach((dispose) => dispose());
    document.body.replaceChildren();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});
function setup(scale = 1) {
    const node = document.createElement('div');
    const button = document.createElement('button');
    node.append(button);
    document.body.append(node);
    Object.defineProperties(node, {
        clientWidth: { value: 300 },
        offsetWidth: { value: 300 },
        scrollWidth: { value: 1000 },
    });
    vi.spyOn(node, 'getBoundingClientRect').mockReturnValue({ width: 300 * scale } as DOMRect);
    node.scrollLeft = 200;
    const captures = new Set<number>();
    node.setPointerCapture = (id) => captures.add(id);
    node.hasPointerCapture = (id) => captures.has(id);
    node.releasePointerCapture = (id) => captures.delete(id);
    const select = vi.fn();
    button.addEventListener('click', select);
    const interact = vi.fn();
    const action = imageStripDrag(node, interact);
    disposals.push(() => action.destroy());
    return { node, button, captures, select, interact, action };
}
function pointer(target: EventTarget, type: string, x: number, y = 80, time = 0, extra = {}) {
    const event = new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        clientX: x,
        clientY: y,
        button: 0,
    });
    for (const [key, value] of Object.entries({
        pointerId: 1,
        isPrimary: true,
        pointerType: 'mouse',
        timeStamp: time,
        ...extra,
    }))
        Object.defineProperty(event, key, { value });
    target.dispatchEvent(event);
    return event;
}
function click(button: HTMLElement, detail = 1) {
    button.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail }));
}
function advance(time: number) {
    const pending = [...frames.values()];
    frames.clear();
    pending.forEach((callback) => callback(time));
}

it('drags from a thumbnail in logical pixels, clamps the strip, and suppresses only the drag click', () => {
    const { node, button, captures, select } = setup(2);
    expect(pointer(button, 'pointerdown', 250).defaultPrevented).toBe(false);
    expect(pointer(window, 'pointermove', 150, 80, 100).defaultPrevented).toBe(true);
    expect(node.scrollLeft).toBe(250);
    expect(captures.has(1)).toBe(true);
    pointer(window, 'pointermove', -2000, 80, 200);
    expect(node.scrollLeft).toBe(700);
    pointer(window, 'pointerup', -2000, 80, 400);
    expect(captures.size).toBe(0);
    expect(node.dataset.stripDragging).toBeUndefined();
    click(button);
    expect(select).not.toHaveBeenCalled();
    pointer(button, 'pointerdown', 250, 80, 500);
    pointer(window, 'pointerup', 250, 80, 520);
    click(button);
    expect(select).toHaveBeenCalledOnce();
});

it('keeps small movements, vertical intent, and keyboard activation as ordinary clicks', () => {
    const { node, button, select } = setup();
    pointer(button, 'pointerdown', 250);
    pointer(window, 'pointermove', 247, 82, 100);
    pointer(window, 'pointerup', 247, 82, 200);
    click(button);
    expect(select).toHaveBeenCalledTimes(1);
    pointer(button, 'pointerdown', 250);
    expect(pointer(window, 'pointermove', 248, 110, 100).defaultPrevented).toBe(false);
    pointer(window, 'pointerup', 248, 110, 200);
    expect(node.scrollLeft).toBe(200);
    pointer(button, 'pointerdown', 250);
    pointer(window, 'pointermove', 150, 80, 100);
    pointer(window, 'pointerup', 150, 80, 300);
    click(button, 0);
    expect(select).toHaveBeenCalledTimes(2);
});

it('leaves touch and wheel scrolling native while interrupting automatic centering', () => {
    const { node, button, captures, interact } = setup();
    const touch = { pointerType: 'touch' };
    expect(pointer(button, 'pointerdown', 250, 80, 0, touch).defaultPrevented).toBe(false);
    expect(pointer(window, 'pointermove', 100, 80, 100, touch).defaultPrevented).toBe(false);
    const wheel = new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaX: 80 });
    node.dispatchEvent(wheel);
    expect(wheel.defaultPrevented).toBe(false);
    expect(captures.size).toBe(0);
    expect(node.scrollLeft).toBe(200);
    expect(interact).toHaveBeenCalledTimes(2);
});

it('coasts after a flick and stops immediately when the strip is touched again', () => {
    const { node, button } = setup();
    pointer(button, 'pointerdown', 250);
    pointer(window, 'pointermove', 150, 80, 50);
    pointer(window, 'pointerup', 150, 80, 60);
    advance(16);
    expect(node.scrollLeft).toBeGreaterThan(300);
    const stopped = node.scrollLeft;
    pointer(button, 'pointerdown', 150, 80, 70);
    expect(frames.size).toBe(0);
    advance(32);
    expect(node.scrollLeft).toBe(stopped);
});

it('does not coast after a paused release or when reduced motion is requested', () => {
    const { node, button } = setup();
    pointer(button, 'pointerdown', 250);
    pointer(window, 'pointermove', 150, 80, 50);
    pointer(window, 'pointerup', 150, 80, 250);
    expect(frames.size).toBe(0);
    vi.stubGlobal('matchMedia', () => ({ matches: true }));
    pointer(button, 'pointerdown', 250);
    pointer(window, 'pointermove', 150, 80, 50);
    pointer(window, 'pointerup', 150, 80, 60);
    expect(frames.size).toBe(0);
    expect(node.scrollLeft).toBe(400);
});

it.each(['pointercancel', 'blur', 'resize', 'lostpointercapture'])(
    'releases capture and scrolling authority after %s',
    (type) => {
        const { node, button, captures, action } = setup();
        pointer(button, 'pointerdown', 250);
        pointer(window, 'pointermove', 150, 80, 50);
        pointer(type === 'lostpointercapture' ? node : window, type, 150);
        expect(captures.size).toBe(0);
        pointer(window, 'pointermove', 0, 80, 80);
        pointer(window, 'pointerup', 0, 80, 100);
        expect(node.scrollLeft).toBe(300);
        expect(frames.size).toBe(0);
        action.destroy();
        pointer(button, 'pointerdown', 250);
        pointer(window, 'pointermove', 150, 80, 50);
        expect(node.scrollLeft).toBe(300);
    },
);
