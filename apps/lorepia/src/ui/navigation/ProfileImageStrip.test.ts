import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { createPreviewClient } from '../../preview/mock-client';
import ProfileImageStrip from './ProfileImageStrip.svelte';

afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});

it('previews the thumbnail crossing center without prop updates interrupting the drag, then snaps and preserves keyboard navigation', async () => {
    const frames = new Map<number, FrameRequestCallback>();
    let id = 0;
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
        frames.set(++id, callback);
        return id;
    });
    vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
    vi.spyOn(performance, 'now').mockReturnValue(0);
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
    const onselect = vi.fn();
    const view = render(ProfileImageStrip, {
        client: createPreviewClient(),
        images: Array.from({ length: 8 }, (_, index) => ({
            assetId: `asset-${String(index)}`,
            title: `Image ${String(index)}`,
        })),
        index: 0,
        label: 'Images',
        onselect,
    });
    const strip = screen.getByRole('toolbar', { name: 'Images' });
    const buttons = [...strip.querySelectorAll('button')];
    Object.defineProperties(strip, {
        clientWidth: { value: 300 },
        offsetWidth: { value: 300 },
        scrollWidth: { value: 664 },
    });
    buttons.forEach((button, index) => {
        Object.defineProperties(button, {
            offsetLeft: { value: 126 + index * 52 },
            offsetWidth: { value: 48 },
        });
    });
    strip.setPointerCapture = vi.fn();
    strip.hasPointerCapture = () => true;
    strip.releasePointerCapture = vi.fn();
    resize?.();
    await view.rerender({ index: 3 });
    await waitFor(() => expect(frames.size).toBeGreaterThan(0));
    const chosen = buttons[3];
    if (!chosen) throw new Error('Missing thumbnail');
    async function pointer(target: HTMLElement | Window, type: string, x: number, time: number) {
        const event = new MouseEvent(type, { bubbles: true, cancelable: true, clientX: x });
        Object.defineProperties(event, {
            pointerId: { value: 1 },
            isPrimary: { value: true },
            pointerType: { value: 'mouse' },
            timeStamp: { value: time },
        });
        await fireEvent(target, event);
    }
    await pointer(chosen, 'pointerdown', 220, 0);
    expect(frames.size).toBe(0);
    await pointer(window, 'pointermove', 120, 100);
    expect(strip.scrollLeft).toBe(100);
    expect(onselect).toHaveBeenLastCalledWith(2, true);
    await view.rerender({ index: 2 });
    expect(strip.scrollLeft).toBe(100);
    expect(frames.size).toBe(0);
    await pointer(window, 'pointerup', 120, 300);
    await fireEvent.click(chosen, { detail: 1 });
    expect(onselect).toHaveBeenCalledTimes(1);
    const pending = [...frames.values()];
    frames.clear();
    pending.forEach((callback) => callback(200));
    expect(strip.scrollLeft).toBe(104);
    expect(onselect).toHaveBeenLastCalledWith(2, false);
    expect(frames.size).toBe(0);
    await fireEvent.keyDown(chosen, { key: 'ArrowRight' });
    expect(onselect).toHaveBeenLastCalledWith(4, false);
    expect(buttons[4]).toHaveFocus();
    view.unmount();
    expect(frames.size).toBe(0);
});
