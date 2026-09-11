import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { scrollHeaderMotion } from './scroll-header-motion';
import { scrollHeaders } from './scroll-headers';

const disposals: (() => void)[] = [];
beforeEach(() => vi.useFakeTimers());
afterEach(() => {
    disposals.splice(0).forEach((dispose) => dispose());
    document.body.replaceChildren();
    vi.useRealTimers();
    vi.restoreAllMocks();
});
function setup(floating = false) {
    const root = document.createElement('div');
    const header = document.createElement('header');
    header.className = 'seed-header';
    if (floating) header.style.position = 'absolute';
    const back = document.createElement('button');
    header.append(back);
    const body = document.createElement('div');
    body.className = 'seed-scroll';
    body.style.paddingTop = '12px';
    root.append(header, body);
    document.body.append(root);
    Object.defineProperty(header, 'offsetHeight', { value: 64 });
    Object.defineProperties(body, {
        scrollHeight: { value: 1600 },
        clientHeight: { value: 700 },
        offsetHeight: { value: 700 },
    });
    return { root, header, body, back };
}
function scroll(body: HTMLElement, top: number) {
    body.scrollTop = top;
    body.dispatchEvent(new Event('scroll'));
}
function attach(header: HTMLElement, body: HTMLElement) {
    const motion = scrollHeaderMotion(header, body);
    disposals.push(() => motion.destroy());
    return motion;
}

it('follows scrolling down and immediately reverses upward, without leaving header space outside the scroller', () => {
    const { root, header, body, back } = setup();
    const motion = attach(header, body);
    expect(body.style.paddingTop).toContain('--ui-scroll-header-height');
    expect(body.style.getPropertyValue('--ui-scroll-body-padding')).toBe('12px');
    expect(root.style.getPropertyValue('--ui-scroll-header-height')).toBe('64px');
    back.focus();
    motion.input();
    scroll(body, 120);
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('64px');
    scroll(body, 95);
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('39px');
    expect(root.style.getPropertyValue('--ui-scroll-header-offset')).toBe('39px');
    vi.advanceTimersByTime(120);
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('0px');
    scroll(body, -40);
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('0px');
});

it('ignores programmatic scroll, hides when browsing search results, and reveals when typing again', () => {
    const { header, body } = setup();
    const motion = attach(header, body);
    scroll(body, 200);
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('0px');
    motion.input();
    scroll(body, 400);
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('64px');
    const input = document.createElement('input');
    header.append(input);
    input.focus();
    scroll(body, 550);
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('0px');
    motion.input();
    scroll(body, 650);
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('64px');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('0px');
    scroll(body, 700);
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('0px');
});

it('changes the compact title only after the original title crosses the header, preserving an edge-to-edge cover', () => {
    const { header, body } = setup(true);
    const heading = document.createElement('h1');
    heading.dataset.scrollTitle = '';
    body.append(heading);
    vi.spyOn(body, 'getBoundingClientRect').mockReturnValue({ top: 0, height: 700 } as DOMRect);
    vi.spyOn(heading, 'getBoundingClientRect').mockImplementation(
        () => ({ bottom: 300 - body.scrollTop }) as DOMRect,
    );
    const motion = attach(header, body);
    expect(header.dataset.titleCollapsed).toBe('false');
    expect(body.style.paddingTop).toBe('12px');
    motion.input();
    scroll(body, 260);
    expect(header.dataset.titleCollapsed).toBe('true');
    scroll(body, 40);
    expect(header.dataset.titleCollapsed).toBe('false');
});

it('reveals navigation for keyboard access and cleans up all presentation state', () => {
    const { root, header, body } = setup();
    const action = scrollHeaders(root);
    disposals.push(() => action.destroy());
    expect(root).toHaveAttribute('data-scroll-header-frame');
    body.dispatchEvent(new WheelEvent('wheel', { bubbles: true, deltaY: 100 }));
    scroll(body, 200);
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('64px');
    body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }));
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('0px');
    action.destroy();
    vi.runAllTimers();
    expect(body.style.paddingTop).toBe('12px');
    expect(header).not.toHaveAttribute('data-scroll-header');
    expect(root).not.toHaveAttribute('data-scroll-header-frame');
    expect(header.style.getPropertyValue('--ui-scroll-header-offset')).toBe('');
    expect(root.style.getPropertyValue('--ui-scroll-header-offset')).toBe('');
    expect(root.style.getPropertyValue('--ui-scroll-header-height')).toBe('');
    expect(root).not.toHaveAttribute('data-scroll-header-frame-settling');
});
