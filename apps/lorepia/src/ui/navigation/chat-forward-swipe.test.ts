import { afterEach, describe, expect, it, vi } from 'vitest';
import { chatForwardSwipe } from './chat-forward-swipe';
import { edgeBack } from '../workspace/edge-back';

const cleanups: (() => void)[] = [];
afterEach(() => {
    cleanups.splice(0).forEach((fn) => fn());
    document.body.replaceChildren();
    vi.restoreAllMocks();
    vi.useRealTimers();
});
function pointer(target: EventTarget, type: string, x: number, y = 100) {
    const event = new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        clientX: x,
        clientY: y,
        button: 0,
    });
    Object.defineProperties(event, { pointerId: { value: 1 }, isPrimary: { value: true } });
    target.dispatchEvent(event);
}
function setup() {
    const host = document.createElement('div');
    host.innerHTML =
        '<section><div class="chat"><textarea></textarea><div data-ui-selectable>Message</div></div></section><section class="right"></section>';
    document.body.append(host);
    const node = host.querySelector<HTMLElement>('.chat');
    const target = host.querySelector<HTMLElement>('.right');
    if (!node || !target) throw new Error('Missing gesture fixture');
    Object.defineProperty(node, 'clientWidth', { value: 393 });
    node.getBoundingClientRect = () => new DOMRect(0, 0, 393, 800);
    const captures = new Set<number>();
    node.setPointerCapture = (id) => {
        captures.add(id);
    };
    node.hasPointerCapture = (id) => captures.has(id);
    node.releasePointerCapture = (id) => {
        captures.delete(id);
        pointer(node, 'lostpointercapture', 0);
    };
    const open = vi.fn();
    const back = edgeBack(node, { onback: vi.fn() });
    const action = chatForwardSwipe(node, { enabled: true, target: () => target, onopen: open });
    cleanups.push(() => {
        action.destroy();
        back.destroy();
    });
    return { node, target, open, action };
}
describe('chat right page gesture', () => {
    it('opens on a leftward edge pan without losing the pointer to the back gesture', async () => {
        const { node, target, open } = setup();
        pointer(node, 'pointerdown', 370);
        pointer(window, 'pointermove', 200);
        expect(target.dataset.forwardPreview).toBe('true');
        expect(target.style.getPropertyValue('--ui-forward-offset')).toBe('223px');
        pointer(window, 'pointerup', 200);
        await Promise.resolve();
        expect(open).toHaveBeenCalledOnce();
    });
    it('keeps vertical scrolling, text selection and composer input in their own regions', () => {
        const { node, target, open } = setup();
        pointer(node, 'pointerdown', 370);
        pointer(window, 'pointermove', 365, 250);
        pointer(window, 'pointerup', 365, 250);
        for (const child of node.querySelectorAll('textarea, [data-ui-selectable]')) {
            pointer(child, 'pointerdown', 200);
            pointer(window, 'pointermove', 40);
            pointer(window, 'pointerup', 40);
        }
        expect(open).not.toHaveBeenCalled();
        expect(target.dataset.forwardPreview).toBeUndefined();
    });
    it('hands trackpad leftward travel to the right page and cleans up cancelled gestures', async () => {
        vi.useFakeTimers();
        const { node, target, open, action } = setup();
        node.dispatchEvent(new WheelEvent('wheel', { deltaX: 140, deltaY: 0, cancelable: true }));
        expect(target.dataset.forwardPreview).toBe('true');
        await vi.advanceTimersByTimeAsync(161);
        expect(open).toHaveBeenCalledOnce();
        pointer(node, 'pointerdown', 370);
        pointer(window, 'pointermove', 200);
        action.update({ enabled: false, target: () => target, onopen: open });
        pointer(window, 'pointerup', 200);
        expect(target.dataset.forwardPreview).toBeUndefined();
        expect(open).toHaveBeenCalledOnce();
    });
});
