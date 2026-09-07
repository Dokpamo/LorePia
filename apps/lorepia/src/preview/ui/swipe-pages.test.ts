import { afterEach, describe, expect, it, vi } from 'vitest';
import { swipePages } from './swipe-pages';

type Options = Parameters<typeof swipePages>[1];
const disposals: (() => void)[] = [];

afterEach(() => {
    disposals.splice(0).forEach((dispose) => dispose());
    document.body.replaceChildren();
    vi.useRealTimers();
});

function setup(overrides: Partial<Options> = {}) {
    const node = document.createElement('div');
    const button = document.createElement('button');
    node.append(button);
    document.body.append(node);
    const navigate = vi.fn();
    const options = {
        enabled: true,
        page: 0,
        subpage: true,
        navigate,
        ...overrides,
    } satisfies Options;
    const captures = new Set<number>();
    Object.defineProperty(node, 'clientWidth', { value: 393 });
    node.getBoundingClientRect = () => new DOMRect(-options.page * 393, 0, 393, 780);
    const capture = vi.fn((id: number) => captures.add(id));
    node.setPointerCapture = capture;
    node.hasPointerCapture = (id: number) => captures.has(id);
    node.releasePointerCapture = vi.fn((id: number) => captures.delete(id));
    const action = swipePages(node, options);
    disposals.push(() => action.destroy());
    return { node, button, navigate, options, action, capture };
}

function pointer(
    target: EventTarget,
    type: string,
    x: number,
    y: number,
    time: number,
    extra: Record<string, number | string | boolean> = {},
) {
    const event = new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        clientX: x,
        clientY: y,
        button: 0,
    });
    const properties = {
        pointerId: 1,
        pointerType: 'touch',
        isPrimary: true,
        timeStamp: time,
        ...extra,
    };
    for (const [key, value] of Object.entries(properties))
        Object.defineProperty(event, key, { value });
    target.dispatchEvent(event);
    return event;
}

const offset = (node: HTMLElement) => node.style.getPropertyValue('--ui-drag-offset');

describe('direct page manipulation', () => {
    it.each([120, 350, 800])('accepts rightward message dragging after %i ms', (delay) => {
        const { node, navigate } = setup({ page: 1 });
        const text = document.createElement('p');
        text.className = 'ui-message';
        node.append(text);
        pointer(text, 'pointerdown', 80, 240, 0, { pointerType: 'mouse' });
        pointer(window, 'pointermove', 240, 240, delay);
        pointer(window, 'pointerup', 240, 240, delay + 40);
        expect(navigate).toHaveBeenCalledExactlyOnceWith(0);
    });
    it('keeps shift, double-click and leftward text selection native', () => {
        const { node, navigate } = setup({ page: 1 });
        const text = document.createElement('p');
        text.className = 'ui-message';
        node.append(text);
        pointer(text, 'pointerdown', 80, 240, 0, { pointerType: 'mouse', shiftKey: true });
        pointer(window, 'pointermove', 240, 240, 100);
        pointer(window, 'pointerup', 240, 240, 140);
        expect(navigate).not.toHaveBeenCalled();
        pointer(text, 'pointerdown', 80, 240, 300, { pointerType: 'mouse', detail: 2 });
        pointer(window, 'pointermove', 240, 240, 650);
        pointer(window, 'pointerup', 240, 240, 700);
        expect(navigate).not.toHaveBeenCalled();
        pointer(text, 'pointerdown', 240, 240, 800, { pointerType: 'mouse' });
        pointer(window, 'pointermove', 80, 240, 920);
        pointer(window, 'pointerup', 80, 240, 960);
        expect(navigate).not.toHaveBeenCalled();
    });
    it('moves back with a trackpad once, including its momentum tail', () => {
        vi.useFakeTimers();
        const { node, navigate } = setup({ page: 2 });
        node.dispatchEvent(new WheelEvent('wheel', { deltaX: 0, deltaY: 0, bubbles: true }));
        node.dispatchEvent(new WheelEvent('wheel', { deltaX: -1, deltaY: 2, bubbles: true }));
        for (const deltaX of [-28, -28, -24, -18, -10, -6, -3, -1]) {
            const event = new WheelEvent('wheel', {
                deltaX,
                deltaY: 1,
                bubbles: true,
                cancelable: true,
            });
            node.dispatchEvent(event);
            expect(event.defaultPrevented).toBe(true);
            vi.advanceTimersByTime(40);
        }
        expect(navigate).not.toHaveBeenCalled();
        vi.advanceTimersByTime(160);
        expect(navigate).toHaveBeenCalledExactlyOnceWith(1);
        expect(offset(node)).toBe('');
    });
    it('preserves vertical and zoom wheel input and cancels a resize during trackpad navigation', () => {
        vi.useFakeTimers();
        const { node, navigate } = setup({ page: 1 });
        const zoom = new WheelEvent('wheel', {
            deltaX: -120,
            ctrlKey: true,
            bubbles: true,
            cancelable: true,
        });
        node.dispatchEvent(zoom);
        expect(zoom.defaultPrevented).toBe(false);
        const vertical = new WheelEvent('wheel', { deltaY: 120, bubbles: true, cancelable: true });
        node.dispatchEvent(vertical);
        expect(vertical.defaultPrevented).toBe(false);
        vi.advanceTimersByTime(200);
        node.dispatchEvent(
            new WheelEvent('wheel', { deltaX: -100, bubbles: true, cancelable: true }),
        );
        window.dispatchEvent(new Event('resize'));
        vi.advanceTimersByTime(300);
        expect(navigate).not.toHaveBeenCalled();
        expect(offset(node)).toBe('');
    });
    it('tracks physical pointer distance correctly when the entire layout is scaled', () => {
        const { node, navigate } = setup({ page: 1 });
        node.getBoundingClientRect = () => new DOMRect(-314.4, 0, 314.4, 624);
        pointer(node, 'pointerdown', 200, 400, 0);
        pointer(window, 'pointermove', 120, 400, 120);
        expect(Number.parseFloat(offset(node)) * 0.8).toBeCloseTo(-80);
        expect(navigate).not.toHaveBeenCalled();
        pointer(window, 'pointerup', 120, 400, 300);
        expect(navigate).toHaveBeenCalledExactlyOnceWith(2);
    });

    it('tracks the pointer before release and settles exactly one page after a long drag', () => {
        const { node, navigate } = setup();
        pointer(node, 'pointerdown', 300, 400, 0);
        pointer(window, 'pointermove', 230, 403, 120);
        expect(offset(node)).toBe('-70px');
        expect(navigate).not.toHaveBeenCalled();
        pointer(window, 'pointermove', 90, 410, 240);
        expect(offset(node)).toBe('-210px');
        pointer(window, 'pointerup', 90, 410, 400);
        expect(navigate).toHaveBeenCalledExactlyOnceWith(1);
        expect(offset(node)).toBe('');
        expect(node.dataset.dragging).toBeUndefined();
        expect(node.hasPointerCapture(1)).toBe(false);
    });

    it('returns a short held drag, but accepts a deliberate short flick', () => {
        const { node, navigate } = setup();
        pointer(node, 'pointerdown', 300, 400, 0);
        pointer(window, 'pointermove', 250, 400, 40);
        pointer(window, 'pointerup', 250, 400, 250);
        expect(navigate).not.toHaveBeenCalled();
        expect(offset(node)).toBe('');
        pointer(node, 'pointerdown', 300, 400, 300);
        pointer(window, 'pointermove', 270, 400, 340);
        pointer(window, 'pointerup', 255, 400, 360);
        expect(navigate).toHaveBeenCalledExactlyOnceWith(1);
    });

    it('lets a deliberate reversal cancel a drag that had crossed the distance threshold', () => {
        const { node, navigate } = setup();
        pointer(node, 'pointerdown', 320, 400, 0);
        pointer(window, 'pointermove', 120, 400, 200);
        pointer(window, 'pointermove', 180, 400, 250);
        pointer(window, 'pointerup', 210, 400, 280);
        expect(navigate).not.toHaveBeenCalled();
    });

    it('keeps vertical scrolling native even if the pointer later moves sideways', () => {
        const { node, navigate, capture } = setup();
        pointer(node, 'pointerdown', 300, 400, 0);
        const vertical = pointer(window, 'pointermove', 298, 380, 20);
        pointer(window, 'pointermove', 100, 360, 80);
        pointer(window, 'pointerup', 100, 360, 100);
        expect(vertical.defaultPrevented).toBe(false);
        expect(capture).not.toHaveBeenCalled();
        expect(offset(node)).toBe('');
        expect(navigate).not.toHaveBeenCalled();
    });

    it.each(['input', 'textarea', 'select'])('preserves %s editing gestures', (tag) => {
        const { node, navigate } = setup();
        const input = document.createElement(tag);
        node.append(input);
        pointer(input, 'pointerdown', 300, 400, 0);
        pointer(window, 'pointermove', 50, 400, 100);
        pointer(window, 'pointerup', 50, 400, 200);
        expect(offset(node)).toBe('');
        expect(navigate).not.toHaveBeenCalled();
    });

    it('allows mouse text selection while allowing a touch drag over the same message', () => {
        const { node, navigate } = setup();
        const message = document.createElement('p');
        message.className = 'ui-message';
        node.append(message);
        pointer(message, 'pointerdown', 300, 400, 0, { pointerType: 'mouse' });
        pointer(window, 'pointermove', 100, 400, 100);
        pointer(window, 'pointerup', 100, 400, 200);
        expect(navigate).not.toHaveBeenCalled();
        pointer(message, 'pointerdown', 300, 400, 300);
        pointer(window, 'pointermove', 100, 400, 400);
        pointer(window, 'pointerup', 100, 400, 500);
        expect(navigate).toHaveBeenCalledExactlyOnceWith(1);
    });

    it('suppresses the click caused by dragging a button, preserving the next tap and keyboard click', () => {
        const { node, button } = setup();
        const activate = vi.fn();
        button.addEventListener('click', activate);
        pointer(button, 'pointerdown', 300, 400, 0);
        pointer(window, 'pointermove', 250, 400, 150);
        pointer(window, 'pointerup', 250, 400, 300);
        button.dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        expect(activate).not.toHaveBeenCalled();
        pointer(button, 'pointerdown', 300, 400, 400);
        pointer(window, 'pointerup', 300, 400, 450);
        button.dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
        button.click();
        expect(activate).toHaveBeenCalledTimes(2);
        expect(offset(node)).toBe('');
    });

    it.each([
        { page: 0, subpage: true, from: 50, to: 300 },
        { page: 1, subpage: false, from: 300, to: 50 },
        { page: 2, subpage: true, from: 300, to: 50 },
    ] as const)('resists the outer edge of page $page with subpage=$subpage', (test) => {
        const { node, navigate } = setup(test);
        pointer(node, 'pointerdown', test.from, 400, 0);
        pointer(window, 'pointermove', test.to, 400, 100);
        const movement = Number.parseFloat(offset(node));
        expect(Math.sign(movement)).toBe(Math.sign(test.to - test.from));
        expect(Math.abs(movement)).toBeLessThan(55);
        pointer(window, 'pointerup', test.to, 400, 200);
        expect(navigate).not.toHaveBeenCalled();
        expect(offset(node)).toBe('');
    });

    it('continues an interrupted settle from the position currently visible', () => {
        const { node, action, options } = setup({ page: 1 });
        node.getBoundingClientRect = () => new DOMRect(-300, 0, 393, 780);
        pointer(node, 'pointerdown', 100, 400, 0);
        pointer(window, 'pointermove', 120, 400, 40);
        expect(offset(node)).toBe('113px');
        action.update({ ...options });
        expect(offset(node)).toBe('113px');
        pointer(window, 'pointermove', 130, 400, 60);
        expect(offset(node)).toBe('123px');
    });

    it.each([
        'pointercancel',
        'lostpointercapture',
        'blur',
        'resize',
        'disable',
        'destroy',
        'multitouch',
    ])('cleans up an interrupted drag on %s', (reason) => {
        const { node, action, options, navigate } = setup();
        pointer(node, 'pointerdown', 300, 400, 0);
        pointer(window, 'pointermove', 100, 400, 100);
        if (reason === 'disable') action.update({ ...options, enabled: false });
        else if (reason === 'destroy') action.destroy();
        else if (reason === 'multitouch')
            pointer(node, 'pointerdown', 200, 400, 120, { pointerId: 2, isPrimary: false });
        else if (reason === 'blur' || reason === 'resize') window.dispatchEvent(new Event(reason));
        else pointer(node, reason, 100, 400, 120);
        pointer(window, 'pointerup', 100, 400, 200);
        expect(navigate).not.toHaveBeenCalled();
        expect(node.hasPointerCapture(1)).toBe(false);
        expect(node.dataset.dragging).toBeUndefined();
        expect(offset(node)).toBe('');
    });
});
