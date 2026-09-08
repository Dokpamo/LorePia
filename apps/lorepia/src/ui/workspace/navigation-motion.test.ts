import { afterEach, describe, expect, it, vi } from 'vitest';
import { navigationUnderlay, pageSlide } from './navigation-motion';

afterEach(() => {
    document.body.replaceChildren();
    vi.restoreAllMocks();
});

function pages() {
    const host = document.createElement('section');
    host.className = 'ui-management';
    host.innerHTML = '<div class="ui-management-content"></div>';
    document.body.append(host);
    function add() {
        const layer = document.createElement('div');
        layer.className = 'ui-overlay-layer';
        const panel = document.createElement('div');
        panel.className = 'ui-overlay';
        layer.append(panel);
        host.append(layer);
        Object.defineProperty(layer, 'clientWidth', { value: 393 });
        return { layer, panel };
    }
    return { host, add };
}

describe('settings page movement', () => {
    it('enters from the right and returns over the same immediate parent', () => {
        const { host, add } = pages();
        const root = add();
        const child = add();
        const enter = pageSlide(child.layer)({ direction: 'in' });
        expect(navigationUnderlay(child.layer)).toBe(root.panel);
        expect(enter.css?.(0)).toBe('transform:translate3d(393px,0,0)');
        expect(enter.css?.(1)).toBe('transform:translate3d(0px,0,0)');
        enter.tick?.(0.5);
        expect(Number.parseFloat(root.panel.style.translate)).toBeCloseTo(-55.02);
        expect((host.firstElementChild as HTMLElement).style.translate).toBe('');
        enter.tick?.(1);
        expect(root.panel.style.translate).toBe('');
        const leave = pageSlide(child.layer)({ direction: 'out' });
        expect(leave.css?.(1)).toBe('transform:translate3d(0px,0,0)');
        expect(leave.css?.(0)).toBe('transform:translate3d(393px,0,0)');
        leave.tick?.(0);
        expect(root.panel.style.translate).toBe('');
    });

    it('does not replay a hidden parent entrance or an already completed interactive back', () => {
        const { add } = pages();
        const root = add();
        root.layer.dataset.navigationCovered = 'true';
        expect(pageSlide(root.layer)({ direction: 'in' }).duration).toBe(0);
        const child = add();
        child.panel.dataset.backDismissed = 'true';
        expect(pageSlide(child.layer)({ direction: 'out' }).duration).toBe(0);
        const next = add();
        expect(navigationUnderlay(next.layer)).toBe(root.panel);
    });

    it('keeps the same navigation destinations with reduced motion', () => {
        const { add } = pages();
        const root = add();
        const child = add();
        const matchMedia = window.matchMedia.bind(window);
        vi.spyOn(window, 'matchMedia').mockImplementation((query) => {
            const result = matchMedia(query);
            Object.defineProperty(result, 'matches', {
                value: query.includes('prefers-reduced-motion'),
            });
            return result;
        });
        expect(pageSlide(child.layer)({ direction: 'in' }).duration).toBe(0);
        expect(pageSlide(child.layer)({ direction: 'out' }).duration).toBe(0);
        expect(navigationUnderlay(child.layer)).toBe(root.panel);
    });
});
