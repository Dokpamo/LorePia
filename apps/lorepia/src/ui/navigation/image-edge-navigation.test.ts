import { afterEach, expect, it, vi } from 'vitest';
import { imageGestures, type ImageGestureOptions } from './image-gestures';
import { edgeBack } from '../workspace/edge-back';

const dispose: (() => void)[] = [];
afterEach(() => {
    dispose.splice(0).forEach((stop) => stop());
    document.body.replaceChildren();
    vi.useRealTimers();
});

function fixture(overrides: Partial<ImageGestureOptions> = {}) {
    const page = document.createElement('section');
    const carousel = document.createElement('div');
    const photo = document.createElement('button');
    carousel.append(photo);
    page.append(carousel);
    document.body.append(page);
    for (const node of [page, carousel]) {
        Object.defineProperty(node, 'clientWidth', { value: 400 });
        node.getBoundingClientRect = () => new DOMRect(0, 0, 400, 800);
        const captured = new Set<number>();
        node.setPointerCapture = (id) => {
            captured.add(id);
        };
        node.hasPointerCapture = (id) => captured.has(id);
        node.releasePointerCapture = (id) => {
            captured.delete(id);
        };
    }
    const onback = vi.fn();
    page.animate = vi.fn(
        () => ({ finished: Promise.resolve(), cancel: vi.fn() }) as unknown as Animation,
    );
    const back = edgeBack(page, { onback });
    const options = {
        next: vi.fn(),
        previous: vi.fn(),
        move: vi.fn(),
        canNext: true,
        canPrevious: false,
        backAtStart: true,
        ...overrides,
    };
    const gesture = imageGestures(carousel, options);
    dispose.push(
        () => gesture.destroy(),
        () => back.destroy(),
    );
    const pointer = (type: string, x: number, time: number, y = 100) => {
        const event = new MouseEvent(type, {
            bubbles: true,
            cancelable: true,
            button: 0,
            clientX: x,
            clientY: y,
        });
        for (const [key, value] of Object.entries({
            pointerId: 1,
            isPrimary: true,
            timeStamp: time,
        }))
            Object.defineProperty(event, key, { value });
        photo.dispatchEvent(event);
        return event;
    };
    return { page, photo, pointer, options, onback };
}

it('hands the first photo to the interactive back gesture, including cancellation', async () => {
    const { page, pointer, onback, options } = fixture();
    expect(pointer('pointerdown', 100, 0).defaultPrevented).toBe(true);
    pointer('pointermove', 125, 200);
    expect(page.style.getPropertyValue('--ui-back-offset')).toBe('25px');
    pointer('pointerup', 125, 400);
    await Promise.resolve();
    expect(onback).not.toHaveBeenCalled();
    pointer('pointerdown', 50, 500);
    pointer('pointermove', 220, 650);
    expect(page.style.getPropertyValue('--ui-back-offset')).toBe('170px');
    expect(options.move).toHaveBeenLastCalledWith(0, 0, false);
    pointer('pointerup', 240, 700);
    await Promise.resolve();
    expect(onback).toHaveBeenCalledOnce();
    expect(options.previous).not.toHaveBeenCalled();
});

it('pages toward the next photo from the first photo without moving the page', () => {
    const { page, pointer, onback, options } = fixture();
    pointer('pointerdown', 300, 0);
    pointer('pointermove', 100, 150);
    expect(page.dataset.backDragging).toBeUndefined();
    expect(options.move).toHaveBeenLastCalledWith(-200, 0, true);
    pointer('pointerup', 100, 200);
    expect(options.next).toHaveBeenCalledOnce();
    expect(onback).not.toHaveBeenCalled();
});

it('resists the last-photo edge and springs back without paging or opening the image', () => {
    const { page, photo, pointer, onback, options } = fixture({
        canNext: false,
        canPrevious: true,
    });
    pointer('pointerdown', 300, 0);
    pointer('pointermove', 100, 150);
    expect(options.move).toHaveBeenLastCalledWith(-24, 0, true);
    expect(page.dataset.backDragging).toBeUndefined();
    pointer('pointerup', 100, 200);
    expect(options.move).toHaveBeenLastCalledWith(0, 0, false);
    expect(options.next).not.toHaveBeenCalled();
    expect(onback).not.toHaveBeenCalled();
    const click = new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 });
    photo.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
});

it('keeps trackpad gesture ownership through a reversal and allows normal back completion', async () => {
    vi.useFakeTimers();
    const { page, photo, options, onback } = fixture();
    const wheel = (deltaX: number) =>
        photo.dispatchEvent(new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaX }));
    wheel(-110);
    expect(page.style.getPropertyValue('--ui-back-offset')).toBe('110px');
    wheel(90);
    expect(page.style.getPropertyValue('--ui-back-offset')).toBe('20px');
    expect(options.move).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(200);
    expect(onback).not.toHaveBeenCalled();
    wheel(-120);
    await vi.advanceTimersByTimeAsync(200);
    expect(onback).toHaveBeenCalledOnce();
});
