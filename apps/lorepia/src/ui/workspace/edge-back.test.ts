import { afterEach, describe, expect, it, vi } from 'vitest';
import { edgeBack, requestBack } from './edge-back';

type Options = Parameters<typeof edgeBack>[1];
const disposals: (() => void)[] = [];

afterEach(() => {
    disposals.splice(0).forEach((dispose) => dispose());
    vi.restoreAllMocks();
    vi.useRealTimers();
    document.body.replaceChildren();
});

function setup(overrides: Partial<Options> = {}) {
    const host = document.createElement('div');
    host.innerHTML = `<div class="ui-management-content"></div>
        <div class="ui-overlay-layer"><div class="ui-overlay" inert></div></div>
        <div class="ui-overlay-layer"><div class="ui-overlay"></div></div>`;
    document.body.append(host);
    const parent = host.querySelector<HTMLElement>('.ui-overlay');
    const node = host.querySelector<HTMLElement>('.ui-overlay-layer:last-child .ui-overlay');
    if (!parent || !node) throw new Error('Missing overlay fixture');
    const captures = new Set<number>();
    Object.defineProperty(node, 'clientWidth', { value: 393 });
    node.getBoundingClientRect = () => new DOMRect(0, 0, 393, 780);
    node.setPointerCapture = (id) => captures.add(id);
    node.hasPointerCapture = (id) => captures.has(id);
    node.releasePointerCapture = (id) => captures.delete(id);
    const animations: {
        target: Element;
        keyframes: Keyframe[];
        finish: () => void;
        cancel: ReturnType<typeof vi.fn>;
    }[] = [];
    vi.spyOn(Element.prototype, 'animate').mockImplementation(function (this: Element, keyframes) {
        let finish!: () => void;
        const finished = new Promise<void>((resolve) => (finish = resolve));
        const cancel = vi.fn();
        animations.push({ target: this, keyframes: keyframes as Keyframe[], finish, cancel });
        return { finished, cancel } as unknown as Animation;
    });
    const onback = vi.fn();
    const options = { onback, ...overrides };
    const action = edgeBack(node, options);
    disposals.push(() => action.destroy());
    function animation(index: number) {
        const result = animations[index];
        if (!result) throw new Error(`Missing animation ${String(index)}`);
        return result;
    }
    return { node, parent, host, animations, animation, onback, options, action };
}

function pointer(target: EventTarget, type: string, x: number, time: number) {
    const event = new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        clientX: x,
        clientY: 100,
        button: 0,
    });
    for (const [key, value] of Object.entries({ pointerId: 1, isPrimary: true, timeStamp: time }))
        Object.defineProperty(event, key, { value });
    target.dispatchEvent(event);
    return event;
}

function drag(node: HTMLElement, distance: number) {
    pointer(node, 'pointerdown', 10, 0);
    pointer(window, 'pointermove', 10 + distance, 300);
    pointer(window, 'pointerup', 10 + distance, 500);
}

describe('overlay back gestures', () => {
    it('finishes leaving before asking about edits, then resumes from the right only on keep', async () => {
        const confirm = vi.fn<(resume: () => Promise<void>) => void>();
        const { node, parent, animation, animations, onback, options, action } = setup({
            beforeback: () => ({ confirm }),
        });
        drag(node, 150);
        expect(animation(0).keyframes.at(-1)?.transform).toBe('translate3d(393px,0,0)');
        expect(confirm).not.toHaveBeenCalled();
        animation(0).finish();
        await Promise.resolve();
        expect(confirm).toHaveBeenCalledOnce();
        expect(onback).not.toHaveBeenCalled();
        expect(parent.style.translate).toBe('');
        expect(node.style.getPropertyValue('--ui-back-offset')).toBe('100%');
        action.update({ ...options, enabled: false });
        window.dispatchEvent(new Event('resize'));
        requestBack(node);
        expect(node.style.getPropertyValue('--ui-back-offset')).toBe('100%');
        expect(animations).toHaveLength(2);
        action.update({ ...options, enabled: true });
        const resume = confirm.mock.calls[0]?.[0];
        if (!resume) throw new Error('Missing resume action');
        const returning = resume();
        expect(animation(2).keyframes).toEqual([
            { transform: 'translate3d(393px,0,0)' },
            { transform: 'translate3d(0px,0,0)' },
        ]);
        animation(2).finish();
        await returning;
        expect(node.dataset.backDismissed).toBeUndefined();
        expect(node.style.getPropertyValue('--ui-back-offset')).toBe('');
        expect(onback).not.toHaveBeenCalled();
    });

    it.each(['input', 'label'])(
        'keeps a starting-situation %s clickable at the back-gesture edge',
        (target) => {
            const { node, onback } = setup();
            const label = document.createElement('label');
            const input = document.createElement('input');
            input.type = 'radio';
            label.append(input);
            node.append(label);
            const control = target === 'input' ? input : label;
            const down = pointer(control, 'pointerdown', 30, 0);
            expect(down.defaultPrevented).toBe(false);
            expect(node.hasPointerCapture(1)).toBe(false);
            pointer(window, 'pointerup', 30, 100);
            control.click();
            expect(input.checked).toBe(true);
            expect(onback).not.toHaveBeenCalled();
        },
    );

    it('preserves an active drag across unchanged enabled options', async () => {
        const { node, animation, onback, options, action } = setup({ enabled: true });
        pointer(node, 'pointerdown', 10, 0);
        pointer(window, 'pointermove', 160, 300);
        action.update({ ...options, enabled: true });
        expect(node.style.getPropertyValue('--ui-back-offset')).toBe('150px');
        pointer(window, 'pointerup', 160, 500);
        animation(0).finish();
        await Promise.resolve();
        expect(onback).toHaveBeenCalledOnce();
    });

    it('reveals the retained settings parent and commits only once across repeated inputs', async () => {
        vi.useFakeTimers();
        const { node, parent, host, animations, animation, onback } = setup();
        pointer(node, 'pointerdown', 10, 0);
        pointer(window, 'pointermove', 160, 300);
        expect(parent.style.translate).not.toBe('');
        expect((host.firstElementChild as HTMLElement).style.translate).toBe('');
        pointer(window, 'pointerup', 160, 500);
        expect(animation(1).target).toBe(parent);
        requestBack(node);
        node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        node.dispatchEvent(new WheelEvent('wheel', { deltaX: -100, cancelable: true }));
        vi.advanceTimersByTime(200);
        expect(animations).toHaveLength(2);
        animation(0).finish();
        await Promise.resolve();
        expect(onback).toHaveBeenCalledOnce();
        expect(parent.style.translate).toBe('');
        expect(node.dataset.backDismissed).toBe('true');
        requestBack(node);
        drag(node, 180);
        expect(animations).toHaveLength(2);
        expect(onback).toHaveBeenCalledOnce();
    });

    it('regrabs a settling page without letting the canceled completion navigate', async () => {
        const { node, animation, onback } = setup();
        requestBack(node);
        const original = getComputedStyle.bind(window);
        const computed = vi
            .spyOn(window, 'getComputedStyle')
            .mockImplementation((element) =>
                element === node
                    ? ({ transform: 'matrix(1, 0, 0, 1, 80, 0)' } as CSSStyleDeclaration)
                    : original(element),
            );
        pointer(node, 'pointerdown', 10, 0);
        computed.mockRestore();
        pointer(window, 'pointermove', 160, 300);
        pointer(window, 'pointerup', 160, 500);
        expect(animation(0).cancel).toHaveBeenCalled();
        animation(0).finish();
        await Promise.resolve();
        expect(onback).not.toHaveBeenCalled();
        animation(2).finish();
        await Promise.resolve();
        expect(onback).toHaveBeenCalledOnce();
    });

    it.each(['rejected', 'short', 'canceled'])(
        'settles a %s drag back and accepts the next gesture',
        async (reason) => {
            const beforeback = vi.fn(() => reason !== 'rejected');
            const { node, parent, animation, onback } = setup({ beforeback });
            if (reason === 'canceled') {
                pointer(node, 'pointerdown', 10, 0);
                pointer(window, 'pointermove', 160, 300);
                pointer(window, 'pointercancel', 160, 400);
            } else drag(node, reason === 'short' ? 20 : 150);
            expect(animation(0).keyframes.at(-1)?.transform).toBe('translate3d(0px,0,0)');
            animation(0).finish();
            await Promise.resolve();
            expect(onback).not.toHaveBeenCalled();
            expect(parent.style.translate).toBe('');
            expect(node.dataset.backSettling).toBeUndefined();
            beforeback.mockReturnValue(true);
            drag(node, 150);
            animation(2).finish();
            await Promise.resolve();
            expect(onback).toHaveBeenCalledOnce();
        },
    );

    it.each(['disable', 'replace', 'destroy'])(
        'invalidates a pending completion on %s',
        async (reason) => {
            const { node, parent, animation, onback, options, action } = setup();
            requestBack(node);
            const replacement = vi.fn();
            if (reason === 'disable') action.update({ ...options, enabled: false });
            else if (reason === 'replace') action.update({ onback: replacement });
            else action.destroy();
            animation(0).finish();
            await Promise.resolve();
            expect(onback).not.toHaveBeenCalled();
            expect(replacement).not.toHaveBeenCalled();
            expect(parent.style.translate).toBe('');
            expect(node.dataset.backSettling).toBeUndefined();
        },
    );
});
