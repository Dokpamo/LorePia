import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { t } from '../../lib/i18n';
import UiPreview from './UiPreview.svelte';

afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

function setup() {
    const app = render(UiPreview);
    const viewport = app.container.querySelector('.ui-viewport');
    if (!viewport) throw new Error('Missing viewport');
    let width = 393;
    let height = 780;
    Object.defineProperties(viewport, {
        clientWidth: { get: () => width },
        clientHeight: { get: () => height },
    });
    const resize = async (nextWidth: number, nextHeight = 780) => {
        width = nextWidth;
        height = nextHeight;
        await fireEvent(window, new Event('resize'));
    };
    return { ...app, resize };
}

describe('UI state across viewport changes', () => {
    it('keeps responsive motion but does not restart it for every pixel resize', async () => {
        vi.useFakeTimers();
        const { resize } = setup();
        await resize(1440);
        await resize(1119);
        const main = screen.getByRole('main');
        expect(main).toHaveAttribute('data-reflowing', 'true');
        await vi.advanceTimersByTimeAsync(80);
        await resize(1080);
        expect(main).toHaveAttribute('data-reflowing', 'true');
        await vi.advanceTimersByTimeAsync(80);
        await resize(1000);
        await vi.advanceTimersByTimeAsync(48);
        await tick();
        expect(main).toHaveAttribute('data-reflowing', 'false');
        expect(main.style.getPropertyValue('--ui-chat-width')).toBe('700px');
    });

    it('retains the focused editor, draft and mounted pages across three, two and one columns', async () => {
        const { container, resize } = setup();
        await resize(1440);
        expect(screen.getByRole('main')).toHaveAttribute('data-layout', 'wide');
        for (const name of [t('uiPreview.management'), t('uiPreview.chat'), t('uiPreview.subpage')])
            expect(screen.getByRole('region', { name })).toBeVisible();
        const input = screen.getByRole('textbox', { name: t('uiPreview.message') });
        input.focus();
        await fireEvent.input(input, { target: { value: '창을 줄여도 이어 쓰기' } });
        await resize(900);
        expect(screen.getByRole('main')).toHaveAttribute('data-layout', 'split');
        expect(screen.getByRole('textbox', { name: t('uiPreview.message') })).toBe(input);
        await resize(393);
        expect(screen.getByRole('main')).toHaveAttribute('data-layout', 'mobile');
        expect(input).toHaveFocus();
        expect(input).toHaveValue('창을 줄여도 이어 쓰기');
        expect(container.querySelector('.ui-chat')).toHaveAttribute('aria-hidden', 'false');
        expect(screen.queryByRole('region', { name: t('uiPreview.management') })).toBeNull();
        expect(container.querySelectorAll('.ui-page')).toHaveLength(3);
        await resize(320, 640);
        const main = screen.getByRole('main');
        expect(main.style.width).toBe('');
        expect(main.style.height).toBe('');
        expect(main.style.transform).toBe('');
        await waitFor(() => expect(main.style.getPropertyValue('--ui-chat-width')).toBe('320px'));
        expect(input).toHaveValue('창을 줄여도 이어 쓰기');
        await resize(393, 748);
        expect(main.style.width).toBe('');
        expect(main.style.height).toBe('');
        expect(main.style.transform).toBe('');
        expect(main.style.left).toBe('');
        expect(main.style.top).toBe('');
        expect(input).toHaveValue('창을 줄여도 이어 쓰기');
    });

    it('keeps creator content paired with chat at medium width and visible on mobile', async () => {
        const { resize } = setup();
        await fireEvent.click(screen.getByRole('button', { name: '비 오는 오후 · 오늘' }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.openSubpage') }));
        await resize(900);
        expect(screen.getByRole('region', { name: t('uiPreview.chat') })).toBeVisible();
        expect(screen.getByRole('region', { name: t('uiPreview.subpage') })).toBeVisible();
        expect(screen.queryByRole('region', { name: t('uiPreview.management') })).toBeNull();
        await resize(393);
        expect(screen.getByRole('region', { name: t('uiPreview.subpage') })).toBeVisible();
        expect(screen.queryByRole('region', { name: t('uiPreview.chat') })).toBeNull();
    });

    it('preserves settings and accepts a swipe immediately after resizing, without a timed lock', async () => {
        vi.useFakeTimers();
        const { container, resize } = setup();
        await resize(1440);
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.cardSettings') }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.cardName') }));
        const name = screen.getByRole('textbox', { name: t('uiPreview.cardName') });
        await fireEvent.input(name, { target: { value: '작은 도서관' } });
        name.focus();
        await resize(393);
        expect(name).toHaveFocus();
        expect(name).toHaveValue('작은 도서관');
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.closeEditor') }));
        await vi.advanceTimersByTimeAsync(250);
        await waitFor(() => expect(screen.queryByRole('textbox')).toBeNull());
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.save') }));
        expect(screen.getByRole('heading', { name: '작은 도서관' })).toBeVisible();

        const track = container.querySelector<HTMLElement>('.ui-track');
        if (!track) throw new Error('Missing track');
        Object.defineProperty(track, 'clientWidth', { value: 393 });
        track.getBoundingClientRect = () => new DOMRect(0, 0, 393, 780);
        track.setPointerCapture = vi.fn();
        track.hasPointerCapture = () => false;
        for (const [type, x, time] of [
            ['pointerdown', 300, 0],
            ['pointermove', 100, 200],
            ['pointerup', 100, 400],
        ] as const) {
            const event = new MouseEvent(type, { bubbles: true, clientX: x, clientY: 400 });
            for (const [key, value] of Object.entries({
                pointerId: 1,
                pointerType: 'touch',
                isPrimary: true,
                timeStamp: time,
            }))
                Object.defineProperty(event, key, { value });
            await fireEvent(track, event);
        }
        expect(screen.getByRole('region', { name: t('uiPreview.chat') })).toBeVisible();
        expect(screen.queryByRole('region', { name: t('uiPreview.management') })).toBeNull();
    });

    it('uses observer measurements immediately and cancels pending frame work on unmount', async () => {
        vi.useFakeTimers();
        const matchMedia = window.matchMedia.bind(window);
        vi.spyOn(window, 'matchMedia').mockImplementation((query) => {
            const media = matchMedia(query);
            Object.defineProperty(media, 'matches', { value: true });
            return media;
        });
        let notify: ResizeObserverCallback = () => undefined;
        const observe = vi.fn();
        const disconnect = vi.fn();
        let observerCount = 0;
        vi.stubGlobal(
            'ResizeObserver',
            class {
                constructor(private callback: ResizeObserverCallback) {
                    observerCount += 1;
                }
                observe(node: Element) {
                    observe(node);
                    if (node.classList.contains('ui-viewport')) notify = this.callback;
                }
                disconnect = disconnect;
            },
        );
        const { container, resize, unmount } = setup();
        const viewport = container.querySelector('.ui-viewport');
        const main = screen.getByRole('main');
        expect(observe.mock.calls.filter(([node]) => node === viewport)).toHaveLength(1);
        await resize(1440);
        // A native window event does not run a second layout measurement.
        expect(main).toHaveAttribute('data-layout', 'mobile');
        for (const width of [1440, 1119, 1120, 759, 760, 320]) {
            notify(
                [{ contentRect: { width, height: 640 } } as ResizeObserverEntry],
                {} as ResizeObserver,
            );
            await tick();
            expect(main).toHaveAttribute(
                'data-layout',
                width >= 1120 ? 'wide' : width >= 760 ? 'split' : 'mobile',
            );
        }
        expect(main.style.width).toBe('');
        expect(main.style.height).toBe('');
        expect(main.style.transform).toBe('');
        expect(main).toHaveAttribute('data-resizing', 'true');
        vi.advanceTimersToNextFrame();
        await tick();
        expect(main).toHaveAttribute('data-resizing', 'false');

        notify(
            [{ contentRect: { width: 900, height: 780 } } as ResizeObserverEntry],
            {} as ResizeObserver,
        );
        await tick();
        expect(main.style.transform).toBe('');
        expect(vi.getTimerCount()).toBeGreaterThan(0);
        unmount();
        expect(disconnect).toHaveBeenCalledTimes(observerCount);
        expect(vi.getTimerCount()).toBe(0);
    });
});
