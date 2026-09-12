import type { BackDecision } from './edge-back';
import { sheetDeparture } from './sheet-departure';

interface Options {
    enabled: boolean;
    backdrop: () => HTMLElement | undefined;
    beforeback?: () => BackDecision;
    onclose: () => void;
}

/** A dirty sheet departs before confirmation, then returns to its retained draft. */
export function settingsSheetBack(panel: HTMLElement, initial: Options) {
    let options = initial;
    let moving = false;
    const departure = sheetDeparture(panel, () => options.backdrop());
    async function close() {
        if (!options.enabled || moving) return;
        const decision = options.beforeback?.() ?? true;
        if (decision === false) return;
        if (decision === true) {
            options.onclose();
            return;
        }
        moving = true;
        const hidden = await departure.hide();
        moving = false;
        if (hidden)
            decision.confirm(async () => {
                await departure.show();
            });
    }
    panel.addEventListener('ui-back', close);
    return {
        update(next: Options) {
            options = next;
        },
        destroy() {
            departure.destroy();
            panel.removeEventListener('ui-back', close);
        },
    };
}
