import { afterEach, describe, expect, it, vi } from 'vitest';
import { choiceSheetDrag, choiceSheetTransition } from './choice-sheet-motion';

const disposals: (() => void)[] = [];
afterEach(() => {
    disposals.splice(0).forEach((dispose) => dispose());
    document.body.replaceChildren();
    vi.restoreAllMocks();
});

function pointer(target: EventTarget, type: string, y: number, time: number, id = 1) {
    const event = new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        clientY: y,
        button: 0,
    });
    for (const [key, value] of Object.entries({
        pointerId: id,
        pointerType: 'touch',
        isPrimary: id === 1,
        timeStamp: time,
    }))
        Object.defineProperty(event, key, { value });
    target.dispatchEvent(event);
    return event;
}

function fixture(scale = 1) {
    const layer = document.createElement('div');
    const panel = document.createElement('div');
    const handle = document.createElement('button');
    const option = document.createElement('button');
    panel.append(handle, option);
    layer.append(panel);
    document.body.append(layer);
    const offset = () => Number.parseFloat(panel.style.getPropertyValue('--ui-choice-drag')) || 0;
    Object.defineProperty(panel, 'offsetHeight', { value: 560 });
    panel.getBoundingClientRect = () =>
        new DOMRect(0, (240 + offset()) * scale, 360 * scale, 560 * scale);
    layer.getBoundingClientRect = () => new DOMRect(0, 0, 360 * scale, 800 * scale);
    const captures = new Set<number>();
    handle.setPointerCapture = (id) => {
        captures.add(id);
    };
    handle.hasPointerCapture = (id) => captures.has(id);
    handle.releasePointerCapture = (id) => {
        captures.delete(id);
    };
    const onclose = vi.fn();
    const action = choiceSheetDrag(handle, { panel: () => panel, onclose });
    disposals.push(() => action.destroy());
    return { panel, handle, option, onclose, captures, offset, action };
}

describe('choice sheet handle', () => {
    it('tracks the pointer at the UI scale, then keeps the released position for its exit', () => {
        const { panel, handle, onclose, captures, offset } = fixture(0.5);
        pointer(handle, 'pointerdown', 150, 0);
        pointer(window, 'pointermove', 240, 400);
        expect(offset()).toBe(180);
        expect(panel.dataset.choiceDragging).toBe('true');
        expect(onclose).not.toHaveBeenCalled();
        pointer(window, 'pointerup', 240, 600);
        expect(onclose).toHaveBeenCalledOnce();
        expect(offset()).toBe(180);
        expect(captures.size).toBe(0);
        expect(panel.dataset.choiceDragging).toBeUndefined();
        pointer(window, 'pointerup', 300, 700);
        expect(onclose).toHaveBeenCalledOnce();
    });

    it('returns a short pull and suppresses its trailing click without suppressing keyboard activation', () => {
        const { handle, onclose, offset } = fixture();
        const click = vi.fn();
        handle.addEventListener('click', click);
        pointer(handle, 'pointerdown', 260, 0);
        pointer(window, 'pointermove', 285, 300);
        pointer(window, 'pointerup', 285, 500);
        expect(offset()).toBe(0);
        expect(onclose).not.toHaveBeenCalled();
        handle.dispatchEvent(
            new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 }),
        );
        expect(click).not.toHaveBeenCalled();
        handle.dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 0 }));
        expect(click).toHaveBeenCalledOnce();
    });

    it('accepts a recent downward flick while an upward movement stays in place', () => {
        const { handle, offset, onclose } = fixture();
        pointer(handle, 'pointerdown', 260, 0);
        pointer(window, 'pointermove', 230, 50);
        expect(offset()).toBe(0);
        pointer(window, 'pointerup', 230, 200);
        expect(onclose).not.toHaveBeenCalled();
        pointer(handle, 'pointerdown', 260, 300);
        pointer(window, 'pointermove', 300, 340);
        pointer(window, 'pointerup', 300, 350);
        expect(onclose).toHaveBeenCalledOnce();
    });

    it.each(['pointercancel', 'resize', 'second-touch'])(
        'returns without dismissing on %s',
        (reason) => {
            const { handle, captures, offset, onclose } = fixture();
            pointer(handle, 'pointerdown', 260, 0);
            pointer(window, 'pointermove', 460, 500);
            if (reason === 'resize') window.dispatchEvent(new Event('resize'));
            else if (reason === 'second-touch') pointer(window, 'pointerdown', 280, 600, 2);
            else pointer(window, 'pointercancel', 460, 600);
            expect(offset()).toBe(0);
            expect(captures.size).toBe(0);
            pointer(window, 'pointerup', 460, 700);
            expect(onclose).not.toHaveBeenCalled();
        },
    );

    it('leaves option scrolling alone and releases its gesture listeners on disposal', () => {
        const { handle, option, action, captures, offset, onclose } = fixture();
        const event = pointer(option, 'pointerdown', 400, 0);
        pointer(window, 'pointermove', 600, 100);
        expect(event.defaultPrevented).toBe(false);
        expect(offset()).toBe(0);
        pointer(handle, 'pointerdown', 260, 200);
        pointer(window, 'pointermove', 400, 300);
        action.destroy();
        expect(captures.size).toBe(0);
        pointer(handle, 'pointerdown', 260, 400);
        pointer(window, 'pointermove', 600, 500);
        pointer(window, 'pointerup', 600, 600);
        expect(onclose).not.toHaveBeenCalled();
        expect(offset()).toBe(0);
    });
});

describe('choice sheet transition', () => {
    it('travels fully below the screen and continues from a partially dragged position', () => {
        const { panel } = fixture();
        const enter = choiceSheetTransition(panel, false);
        expect(enter.css?.(0, 1)).toContain('560px');
        panel.style.setProperty('--ui-choice-drag', '140px');
        const exit = choiceSheetTransition(panel, false);
        expect(exit.css?.(0, 1)).toContain('420px');
        expect(exit.css?.(1, 0)).toContain('0px');
        expect(choiceSheetTransition(panel, true).duration).toBe(0);
    });
});
