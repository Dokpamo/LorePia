import { afterEach, describe, expect, it, vi } from 'vitest';
import { sheetDeparture } from './sheet-departure';

afterEach(() => {
    document.body.replaceChildren();
    vi.restoreAllMocks();
});

function fixture() {
    const panel = document.createElement('div');
    const backdrop = document.createElement('div');
    document.body.append(backdrop, panel);
    Object.defineProperty(panel, 'offsetHeight', { value: 560 });
    panel.style.setProperty('--ui-choice-drag', '120px');
    const animations: Animation[] = [];
    const animate = vi.spyOn(panel, 'animate').mockImplementation(() => {
        const animation = { cancel: vi.fn(), onfinish: null } as unknown as Animation;
        animations.push(animation);
        return animation;
    });
    const departure = sheetDeparture(panel, () => backdrop);
    return { panel, backdrop, animate, departure, animations };
}

describe('text sheet deferred dismissal', () => {
    it('finishes departure from the released offset before confirming, then returns the same sheet', async () => {
        const { panel, backdrop, animate, departure, animations } = fixture();
        let settled = false;
        const hiding = departure.hide().then((done) => (settled = done));
        expect(animate.mock.calls[0]?.[0]).toEqual([
            { translate: '0 120px' },
            { translate: '0 560px' },
        ]);
        expect(settled).toBe(false);
        animations[0]?.onfinish?.call(animations[0], new Event('finish') as AnimationPlaybackEvent);
        await hiding;
        expect(settled).toBe(true);
        expect(panel.style.getPropertyValue('--ui-choice-drag')).toBe('100%');
        expect(backdrop.style.opacity).toBe('0');
        expect(panel.dataset.choiceClosing).toBe('true');
        const showing = departure.show();
        expect(animate.mock.calls[1]?.[0]).toEqual([
            { translate: '0 560px' },
            { translate: '0 0px' },
        ]);
        animations[1]?.onfinish?.call(animations[1], new Event('finish') as AnimationPlaybackEvent);
        expect(await showing).toBe(true);
        expect(panel.style.getPropertyValue('--ui-choice-drag')).toBe('0px');
        expect(backdrop.style.opacity).toBe('1');
        expect(panel.dataset.choiceClosing).toBeUndefined();
    });

    it('does not finish a pending dismissal after the editor is destroyed', async () => {
        const { departure, animations } = fixture();
        const hiding = departure.hide();
        departure.destroy();
        expect(await hiding).toBe(false);
        expect(animations[0]?.onfinish).toBeNull();
    });

    it('honors reduced motion without leaving a transition or animation behind', async () => {
        vi.spyOn(window, 'matchMedia').mockReturnValue({ matches: true } as MediaQueryList);
        const { panel, animate, departure } = fixture();
        expect(await departure.hide()).toBe(true);
        expect(await departure.show()).toBe(true);
        expect(animate).not.toHaveBeenCalled();
        expect(panel.style.transition).toBe('');
        expect(panel.style.getPropertyValue('--ui-choice-drag')).toBe('0px');
    });
});
