import { afterEach, describe, expect, it, vi } from 'vitest';
import { measureComposer } from './composer-measure';

const disposals: (() => void)[] = [];
afterEach(() => {
    disposals.splice(0).forEach((dispose) => dispose());
    document.body.replaceChildren();
    vi.useRealTimers();
    vi.unstubAllGlobals();
});
function setup(ondock = vi.fn()) {
    document.body.innerHTML =
        '<div class="ui-chat"><div class="ui-messages"></div><form class="ui-compose"><div class="ui-compose-field"><textarea style="line-height:25.6px;padding:8px 8px 0"></textarea></div></form></div>';
    const input = document.querySelector('textarea');
    const field = document.querySelector<HTMLElement>('.ui-compose-field');
    const chat = document.querySelector<HTMLElement>('.ui-chat');
    if (!input || !field || !chat) throw new Error('Missing composer fixture');
    let lines = 1;
    Object.defineProperty(input, 'scrollHeight', {
        configurable: true,
        get: () => Math.ceil(lines * 25.6 + 8),
    });
    Object.defineProperty(chat, 'clientHeight', { configurable: true, value: 780 });
    const report = vi.fn();
    const action = measureComposer(input, { value: '', onmeasure: report, ondock });
    disposals.push(() => action.destroy());
    const grow = (next: number) => {
        lines = next;
        input.dispatchEvent(new Event('input'));
    };
    return { input, field, chat, report, grow };
}
describe('inline composer line growth', () => {
    it('defers observer-driven sizing to a frame and coalesces repeated resize notifications', async () => {
        vi.useFakeTimers();
        let notify: ResizeObserverCallback | undefined;
        const observer: ResizeObserver = {
            observe: vi.fn(),
            unobserve: vi.fn(),
            disconnect: vi.fn(),
        };
        vi.stubGlobal(
            'ResizeObserver',
            vi.fn(function (callback: ResizeObserverCallback) {
                notify = callback;
                return observer;
            }),
        );
        const { input, field } = setup();
        await Promise.resolve();
        expect(field.style.getPropertyValue('--ui-compose-text-size')).toBe('34px');
        Object.defineProperty(input, 'scrollHeight', { get: () => 60 });
        const dock = field.closest('form');
        if (!notify || !dock) throw new Error('Missing resize observer');
        const entry = (width: number): ResizeObserverEntry => ({
            target: dock,
            contentRect: new DOMRect(0, 0, width, 100),
            borderBoxSize: [],
            contentBoxSize: [],
            devicePixelContentBoxSize: [],
        });
        notify([entry(320), entry(330)], observer);
        await Promise.resolve();
        expect(field.style.getPropertyValue('--ui-compose-text-size')).toBe('34px');
        vi.advanceTimersToNextFrame();
        expect(field.style.getPropertyValue('--ui-compose-text-size')).toBe('60px');
        notify([entry(340)], observer);
        disposals.splice(0).forEach((dispose) => dispose());
        expect(vi.getTimerCount()).toBe(0);
    });
    it('measures at the writing width without moving the text region or leaving overrides behind', () => {
        const { input, field, grow } = setup();
        Object.defineProperty(field, 'clientWidth', { value: 370 });
        input.style.width = '100%';
        input.style.whiteSpace = 'nowrap';
        input.style.padding = '11px 8px 0';
        Object.defineProperty(input, 'scrollHeight', {
            configurable: true,
            get: () => {
                expect(input.style.width).toBe('354px');
                expect(input.style.whiteSpace).toBe('pre-wrap');
                expect(field.classList).not.toContain('ui-compose-measuring');
                return 60;
            },
        });
        grow(2);
        expect(input.style.width).toBe('100%');
        expect(input.style.whiteSpace).toBe('nowrap');
        expect(input.style.padding).toBe('11px 8px 0px');
        expect(field.style.getPropertyValue('--ui-compose-text-size')).toBe('60px');
    });
    it('accounts for the full dock inset when its bottom spacing changes at a fixed field height', () => {
        const ondock = vi.fn();
        const { input, field, chat, grow } = setup(ondock);
        const dock = field.closest<HTMLElement>('form');
        if (!dock) throw new Error('Missing dock');
        Object.defineProperty(field, 'offsetHeight', { value: 120 });
        dock.style.padding = '8px 12px 12px';
        input.focus();
        grow(2);
        expect(chat.style.getPropertyValue('--ui-composer-overlay')).toBe('140px');
        expect(chat.style.getPropertyValue('--ui-composer-bottom')).toBe('12px');
        dock.style.paddingBottom = '34px';
        grow(2);
        expect(chat.style.getPropertyValue('--ui-composer-overlay')).toBe('162px');
        expect(ondock).toHaveBeenLastCalledWith({ delta: 22, follow: true, editing: true });
        disposals.splice(0).forEach((dispose) => dispose());
        expect(chat.style.getPropertyValue('--ui-composer-overlay')).toBe('');
    });
    it('grows from one through ten rendered lines without scrolling, capping only after ten', () => {
        const { input, field, report, grow } = setup();
        for (let lines = 1; lines <= 10; lines++) {
            input.scrollTop = 2;
            grow(lines);
            expect(report).toHaveBeenLastCalledWith({ lines, overflows: false });
            expect(field.style.getPropertyValue('--ui-compose-text-size')).toBe(
                `${String(Math.ceil(lines * 25.6 + 8))}px`,
            );
            expect(input.scrollTop).toBe(0);
        }
        grow(11);
        expect(report).toHaveBeenLastCalledWith({ lines: 11, overflows: true });
        expect(field.style.getPropertyValue('--ui-compose-text-size')).toBe('264px');
        grow(2);
        expect(report).toHaveBeenLastCalledWith({ lines: 2, overflows: false });
        expect(field.style.getPropertyValue('--ui-compose-text-size')).toBe('60px');
    });
    it('re-pins WebKit caret scrolling during the opening frames and stops after disposal', () => {
        vi.useFakeTimers();
        const { input, grow } = setup();
        grow(10);
        vi.runAllTicks();
        for (let frame = 0; frame < 3; frame++) {
            input.scrollTop = 2;
            vi.advanceTimersToNextFrame();
            expect(input.scrollTop).toBe(0);
        }
        disposals.splice(0).forEach((dispose) => dispose());
        expect(vi.getTimerCount()).toBe(0);
    });
    it('caps the text area to leave navigation and tools reachable in a short viewport', () => {
        const { chat, field, grow, report } = setup();
        Object.defineProperty(chat, 'clientHeight', { value: 300 });
        grow(10);
        expect(
            Number.parseFloat(field.style.getPropertyValue('--ui-compose-limit')),
        ).toBeLessThanOrEqual(208);
        expect(report).toHaveBeenLastCalledWith({ lines: 10, overflows: true });
    });
});
