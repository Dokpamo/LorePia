import { afterEach, expect, it, vi } from 'vitest';
import { imageGestures } from './image-gestures';

afterEach(() => document.body.replaceChildren());

it('keeps photo paging separate from downward dismissal, cancels short pans, and suppresses drag clicks', () => {
    const node = document.createElement('div');
    document.body.append(node);
    Object.defineProperty(node, 'clientWidth', { value: 400 });
    node.setPointerCapture = vi.fn();
    node.hasPointerCapture = () => true;
    node.releasePointerCapture = vi.fn();
    const next = vi.fn();
    const previous = vi.fn();
    const dismiss = vi.fn();
    const move = vi.fn();
    const action = imageGestures(node, { next, previous, dismiss, move });
    function pointer(type: string, x: number, y: number, time: number) {
        const event = new Event(type, { bubbles: true, cancelable: true });
        Object.defineProperties(event, {
            pointerId: { value: 1 },
            isPrimary: { value: true },
            button: { value: 0 },
            clientX: { value: x },
            clientY: { value: y },
            timeStamp: { value: time },
        });
        node.dispatchEvent(event);
    }
    pointer('pointerdown', 250, 100, 0);
    pointer('pointermove', 100, 105, 180);
    pointer('pointerup', 100, 105, 240);
    expect(next).toHaveBeenCalledOnce();
    expect(dismiss).not.toHaveBeenCalled();
    const click = new MouseEvent('click', { bubbles: true, cancelable: true });
    node.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
    pointer('pointerdown', 100, 100, 300);
    pointer('pointermove', 108, 240, 600);
    pointer('pointerup', 108, 240, 700);
    expect(dismiss).toHaveBeenCalledOnce();
    expect(previous).not.toHaveBeenCalled();
    pointer('pointerdown', 100, 100, 800);
    pointer('pointermove', 125, 102, 1000);
    pointer('pointercancel', 125, 102, 1100);
    pointer('pointerup', 125, 102, 1200);
    expect(previous).not.toHaveBeenCalled();
    expect(move).toHaveBeenLastCalledWith(0, 0, false);
    action.destroy();
});
