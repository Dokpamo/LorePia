import { afterEach, describe, expect, it, vi } from 'vitest';
import { messagePress } from './message-press';

const disposals: (() => void)[] = [];
afterEach(() => {
    disposals.splice(0).forEach((dispose) => dispose());
    document.body.replaceChildren();
    vi.useRealTimers();
});
function setup() {
    vi.useFakeTimers();
    const node = document.createElement('div');
    document.body.append(node);
    const open = vi.fn();
    const action = messagePress(node, open);
    disposals.push(() => action.destroy());
    return { node, open, action };
}
function pointer(target: EventTarget, type: string, extra: Record<string, unknown> = {}) {
    target.dispatchEvent(
        Object.assign(new Event(type, { bubbles: true, cancelable: true }), {
            pointerId: 1,
            isPrimary: true,
            button: 0,
            clientX: 60,
            clientY: 120,
            ...extra,
        }),
    );
}
describe('message long press', () => {
    it('ignores taps, opens once after holding, and consumes the following click', () => {
        const { node, open } = setup();
        pointer(node, 'pointerdown');
        vi.advanceTimersByTime(100);
        pointer(node, 'pointerup');
        vi.advanceTimersByTime(500);
        expect(open).not.toHaveBeenCalled();
        pointer(node, 'pointerdown');
        vi.advanceTimersByTime(449);
        expect(open).not.toHaveBeenCalled();
        vi.advanceTimersByTime(1);
        expect(open).toHaveBeenCalledTimes(1);
        const click = new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 });
        node.dispatchEvent(click);
        expect(click.defaultPrevented).toBe(true);
    });
    it.each(['pointermove', 'pointercancel', 'scroll'])(
        'cancels a hold on %s and leaves scrolling usable',
        (type) => {
            const { node, open } = setup();
            pointer(node, 'pointerdown');
            pointer(window, type, { clientY: 140 });
            vi.advanceTimersByTime(600);
            expect(open).not.toHaveBeenCalled();
        },
    );
    it('preserves embedded links, supports right click, and clears pending timers on teardown', () => {
        const { node, open, action } = setup();
        const link = document.createElement('a');
        node.append(link);
        pointer(link, 'pointerdown');
        vi.advanceTimersByTime(500);
        expect(open).not.toHaveBeenCalled();
        node.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
        expect(open).toHaveBeenCalledTimes(1);
        pointer(node, 'pointerdown');
        action.destroy();
        vi.advanceTimersByTime(500);
        expect(open).toHaveBeenCalledTimes(1);
    });
});
