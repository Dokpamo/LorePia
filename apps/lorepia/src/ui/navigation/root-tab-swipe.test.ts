import { afterEach, describe, expect, it, vi } from 'vitest';
import { rootTabSwipe } from './root-tab-swipe';

const disposals: (() => void)[] = [];
afterEach(() => {
    disposals.splice(0).forEach((dispose) => dispose());
    document.body.replaceChildren();
    vi.useRealTimers();
});
function setup(index = 0) {
    vi.useFakeTimers();
    const node = document.createElement('div');
    node.innerHTML =
        '<section class="seed-root" data-root-tab="home"><button>Card</button></section>';
    document.body.append(node);
    Object.defineProperty(node, 'clientWidth', { value: 393 });
    const captures = new Set<number>();
    node.setPointerCapture = (id) => captures.add(id);
    node.hasPointerCapture = (id) => captures.has(id);
    node.releasePointerCapture = (id) => captures.delete(id);
    const options = { enabled: true, index, count: 4, prepare: vi.fn(), select: vi.fn() };
    const action = rootTabSwipe(node, options);
    disposals.push(() => action.destroy());
    return { node, options, action };
}
function pointer(target: EventTarget, type: string, x: number, y = 100, time = 0, extra = {}) {
    const event = new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        clientX: x,
        clientY: y,
        button: 0,
        detail: 1,
    });
    for (const [key, value] of Object.entries({
        pointerId: 1,
        isPrimary: true,
        timeStamp: time,
        ...extra,
    }))
        Object.defineProperty(event, key, { value });
    target.dispatchEvent(event);
    return event;
}
function drag(node: HTMLElement, distance: number) {
    pointer(node, 'pointerdown', 200);
    pointer(window, 'pointermove', 200 + distance, 100, 300);
    pointer(window, 'pointerup', 200 + distance, 100, 600);
}

describe('root tab swiping', () => {
    it.each([
        [0, -150, 1],
        [1, -150, 2],
        [2, -150, 3],
        [3, 150, 2],
        [2, 150, 1],
        [1, 150, 0],
    ])('moves one adjacent tab from %i after a %ipx pan', (index, distance, next) => {
        const { node, options } = setup(index);
        drag(node, distance);
        expect(options.prepare).toHaveBeenCalledWith(next);
        expect(options.select).toHaveBeenCalledExactlyOnceWith(next);
    });
    it.each([
        [0, 160],
        [3, -160],
        [1, 20],
    ])('settles boundary or short pans without navigating (%i, %i)', (index, distance) => {
        const { node, options } = setup(index);
        drag(node, distance);
        expect(options.select).not.toHaveBeenCalled();
        expect(node.style.getPropertyValue('--seed-tab-drag')).toBe('0px');
        vi.advanceTimersByTime(300);
        expect(node.dataset.rootSettling).toBeUndefined();
    });
    it('preserves vertical scrolling and taps, and suppresses a card click only after a horizontal drag', () => {
        const { node, options } = setup();
        const button = node.querySelector('button');
        if (!button) throw new Error('Missing card');
        pointer(button, 'pointerdown', 200);
        const vertical = pointer(window, 'pointermove', 190, 150, 100);
        pointer(window, 'pointerup', 40, 200, 200);
        expect(vertical.defaultPrevented).toBe(false);
        expect(options.select).not.toHaveBeenCalled();
        const tap = new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 });
        button.dispatchEvent(tap);
        expect(tap.defaultPrevented).toBe(false);
        drag(node, -150);
        const click = new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 });
        button.dispatchEvent(click);
        expect(click.defaultPrevented).toBe(true);
    });
    it.each(['input', 'textarea', '[data-ui-no-swipe]'])(
        'leaves %s interaction to the nested control',
        (selector) => {
            const { node, options } = setup();
            const target = document.createElement(selector.startsWith('[') ? 'div' : selector);
            if (selector.startsWith('[')) target.dataset.uiNoSwipe = '';
            node.append(target);
            drag(target, -180);
            expect(options.prepare).not.toHaveBeenCalled();
            expect(options.select).not.toHaveBeenCalled();
        },
    );
    it('cancels an in-flight pan when a nested page opens and on a second touch', () => {
        const { node, options, action } = setup();
        pointer(node, 'pointerdown', 200);
        pointer(window, 'pointermove', 50, 100, 300);
        action.update({ ...options, enabled: false });
        pointer(window, 'pointerup', 50, 100, 500);
        expect(options.select).not.toHaveBeenCalled();
        expect(node.dataset.rootDragging).toBeUndefined();
        action.update(options);
        pointer(node, 'pointerdown', 200);
        pointer(window, 'pointermove', 100, 100, 300);
        pointer(node, 'pointerdown', 220, 100, 400, { isPrimary: false, pointerId: 2 });
        expect(node.style.getPropertyValue('--seed-tab-drag')).toBe('0px');
        expect(options.select).not.toHaveBeenCalled();
    });
    it('accepts horizontal trackpad pans once per gesture but leaves vertical scrolling alone', () => {
        const { node, options } = setup();
        node.dispatchEvent(
            new WheelEvent('wheel', { deltaY: 180, bubbles: true, cancelable: true }),
        );
        vi.advanceTimersByTime(200);
        expect(options.select).not.toHaveBeenCalled();
        node.dispatchEvent(
            new WheelEvent('wheel', { deltaX: 160, bubbles: true, cancelable: true }),
        );
        vi.advanceTimersByTime(200);
        expect(options.select).toHaveBeenCalledExactlyOnceWith(1);
    });
});
