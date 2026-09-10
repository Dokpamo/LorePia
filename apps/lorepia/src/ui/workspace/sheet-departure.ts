import { dragOffset } from './choice-sheet-motion';

/** Keep a dirty draft mounted below the screen while its confirmation is shown. */
export function sheetDeparture(panel: HTMLElement, backdrop: () => HTMLElement | undefined) {
    let hidden = false;
    let cancel: (() => void) | undefined;

    function move(hide: boolean) {
        cancel?.();
        const height = panel.offsetHeight;
        const from = hidden ? height : dragOffset(panel);
        const to = hide ? height : 0;
        const shade = backdrop();
        const opacity = shade ? getComputedStyle(shade).opacity : '1';
        panel.dataset.choiceClosing = 'true';
        panel.style.transition = 'none';
        const finish = () => {
            hidden = hide;
            // Percentages keep a hidden draft offscreen when the window is resized.
            panel.style.setProperty('--ui-choice-drag', hide ? '100%' : '0px');
            if (shade) shade.style.opacity = hide ? '0' : '1';
            panel.style.removeProperty('transition');
            if (!hide) delete panel.dataset.choiceClosing;
        };
        if (!height || matchMedia('(prefers-reduced-motion: reduce)').matches) {
            finish();
            return Promise.resolve(true);
        }
        return new Promise<boolean>((resolve) => {
            const timing = {
                duration: 280,
                easing: 'cubic-bezier(0.2, 0.8, 0.2, 1)',
                fill: 'both' as const,
            };
            const motion = panel.animate(
                [{ translate: `0 ${String(from)}px` }, { translate: `0 ${String(to)}px` }],
                timing,
            );
            const fade = shade?.animate([{ opacity }, { opacity: hide ? 0 : 1 }], timing);
            cancel = () => {
                motion.onfinish = null;
                motion.cancel();
                fade?.cancel();
                cancel = undefined;
                resolve(false);
            };
            motion.onfinish = () => {
                finish();
                motion.cancel();
                fade?.cancel();
                cancel = undefined;
                resolve(true);
            };
        });
    }
    return {
        hide: () => move(true),
        show: () => move(false),
        destroy: () => cancel?.(),
    };
}
