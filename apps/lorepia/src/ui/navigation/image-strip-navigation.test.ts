import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { imageStripNavigation } from './image-strip-navigation';

let frames: Map<number, FrameRequestCallback>;
const disposals: (() => void)[] = [];
beforeEach(() => {
    vi.useFakeTimers();
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
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});
function setup(initial = 3) {
    const node = document.createElement('div');
    document.body.append(node);
    Object.defineProperties(node, {
        clientWidth: { value: 300, configurable: true },
        offsetWidth: { value: 300 },
        scrollWidth: { value: 664, configurable: true },
    });
    const buttons = Array.from({ length: 8 }, (_, index) => {
        const button = document.createElement('button');
        Object.defineProperties(button, {
            offsetLeft: { get: () => node.clientWidth / 2 - 24 + index * 52 },
            offsetWidth: { value: 48 },
        });
        node.append(button);
        return button;
    });
    let resize: (() => void) | undefined;
    vi.stubGlobal(
        'ResizeObserver',
        class {
            constructor(callback: () => void) {
                resize = callback;
            }
            observe = vi.fn();
            disconnect = vi.fn();
        },
    );
    const select = vi.fn();
    const navigation = imageStripNavigation(node, initial, select);
    disposals.push(() => navigation.destroy());
    return { node, buttons, select, navigation, resize: () => resize?.() };
}
function advance(time = 200) {
    const pending = [...frames.values()];
    frames.clear();
    pending.forEach((callback) => callback(time));
}
function scroll(node: HTMLElement, position: number) {
    node.scrollLeft = position;
    node.dispatchEvent(new Event('scroll'));
}

it('keeps the first and last thumbnail centered, including after viewport resizing', () => {
    const { node, buttons, navigation, resize } = setup(0);
    expect(node.scrollLeft).toBe(0);
    expect(buttons[0]?.style.getPropertyValue('--image-focus')).toBe('1.000');
    navigation.choose(7);
    advance();
    expect(node.scrollLeft).toBe(364);
    const last = buttons[7];
    if (!last) throw new Error('Missing last thumbnail');
    expect(last.offsetLeft + 24 - node.scrollLeft).toBe(node.clientWidth / 2);
    Object.defineProperties(node, { clientWidth: { value: 500 }, scrollWidth: { value: 864 } });
    resize();
    expect(last.offsetLeft + 24 - node.scrollLeft).toBe(250);
});

it('previews native scrolling at center, ignores its prop echo, then snaps and finalizes once', () => {
    const { node, buttons, select, navigation } = setup();
    buttons[3]?.focus();
    node.dispatchEvent(new WheelEvent('wheel', { deltaX: 73 }));
    scroll(node, 229);
    expect(select).toHaveBeenLastCalledWith(4, true);
    expect(buttons[4]).toHaveFocus();
    navigation.sync(4);
    expect(node.scrollLeft).toBe(229);
    expect(frames.size).toBe(0);
    vi.advanceTimersByTime(120);
    advance();
    expect(node.scrollLeft).toBe(208);
    expect(select.mock.calls).toEqual([
        [4, true],
        [4, false],
    ]);
    node.dispatchEvent(new Event('scrollend'));
    expect(select).toHaveBeenCalledTimes(2);
});

it('waits while a touch is held and lets subsequent native momentum finish before snapping', () => {
    const { node, select } = setup();
    node.dispatchEvent(new Event('pointerdown'));
    node.dispatchEvent(new Event('touchstart'));
    scroll(node, 192);
    vi.advanceTimersByTime(150);
    expect(frames.size).toBe(0);
    node.dispatchEvent(new Event('touchend'));
    vi.advanceTimersByTime(80);
    scroll(node, 243);
    vi.advanceTimersByTime(80);
    expect(frames.size).toBe(0);
    vi.advanceTimersByTime(40);
    advance();
    expect(node.scrollLeft).toBe(260);
    expect(select).toHaveBeenLastCalledWith(5, false);
});

it('does not browse intermediate items during click or external-image centering, and supports reduced motion', () => {
    const { node, select, navigation } = setup(0);
    navigation.choose(7);
    advance(90);
    node.dispatchEvent(new Event('scroll'));
    expect(select.mock.calls).toEqual([[7, false]]);
    advance();
    navigation.sync(2);
    advance();
    expect(node.scrollLeft).toBe(104);
    expect(select.mock.calls).toEqual([[7, false]]);
    vi.stubGlobal('matchMedia', () => ({ matches: true }));
    node.dispatchEvent(new WheelEvent('wheel'));
    scroll(node, 135);
    vi.advanceTimersByTime(120);
    expect(node.scrollLeft).toBe(156);
    expect(frames.size).toBe(0);
    expect(select).toHaveBeenLastCalledWith(3, false);
});

it('cleans up pending settling and motion without sending a late selection', () => {
    const { node, select, navigation } = setup();
    node.dispatchEvent(new WheelEvent('wheel'));
    scroll(node, 225);
    navigation.destroy();
    vi.runAllTimers();
    advance();
    scroll(node, 350);
    expect(select.mock.calls).toEqual([[4, true]]);
    expect(frames.size).toBe(0);
});

it('does not mistake layout scrolling on resize for a new user selection', () => {
    const { node, select, navigation } = setup(1);
    Object.defineProperties(node, { clientWidth: { value: 700 }, scrollWidth: { value: 1064 } });
    window.dispatchEvent(new Event('resize'));
    expect(node.scrollLeft).toBe(52);
    scroll(node, 208);
    vi.advanceTimersByTime(150);
    advance();
    expect(select).not.toHaveBeenCalled();
    navigation.sync(1, true);
    advance();
    expect(node.scrollLeft).toBe(52);
});
