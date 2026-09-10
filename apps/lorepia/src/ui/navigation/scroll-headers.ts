import { scrollHeaderMotion } from './scroll-header-motion';

const bodies = '.seed-scroll, .ui-overlay-body, .ui-messages, .ui-sub-body';

/** Page navigation headers share one behavior; sheet handles and image gestures keep their own owners. */
export function scrollHeaders(root: HTMLElement) {
    const active = new Map<HTMLElement, ReturnType<typeof scrollHeaderMotion>>();
    let frame = 0;
    function sync() {
        frame = 0;
        for (const [body, motion] of active)
            if (!root.contains(body)) {
                motion.destroy();
                active.delete(body);
            }
        for (const body of root.querySelectorAll<HTMLElement>(bodies)) {
            if (active.has(body)) continue;
            const header = body.parentElement?.querySelector<HTMLElement>(':scope > header');
            if (!header?.matches('.seed-header, .ui-page-header')) continue;
            active.set(body, scrollHeaderMotion(header, body));
        }
    }
    function input(event: Event) {
        if (!(event.target instanceof Element)) return;
        if (
            typeof PointerEvent !== 'undefined' &&
            event instanceof PointerEvent &&
            event.type === 'pointermove' &&
            !event.buttons
        )
            return;
        const body = event.target.closest<HTMLElement>(bodies);
        if (body) active.get(body)?.input();
    }
    function keyboard(event: KeyboardEvent) {
        if (event.key === 'Tab') for (const motion of active.values()) motion.show();
        else if (
            ['ArrowDown', 'ArrowUp', 'PageDown', 'PageUp', 'Home', 'End', ' '].includes(event.key)
        )
            input(event);
    }
    const observer = new MutationObserver(() => {
        if (!frame) frame = requestAnimationFrame(sync);
    });
    observer.observe(root, { childList: true, subtree: true });
    for (const type of ['wheel', 'pointerdown', 'pointermove', 'touchmove'])
        root.addEventListener(type, input, { capture: true, passive: true });
    root.addEventListener('keydown', keyboard, true);
    sync();
    return {
        destroy() {
            observer.disconnect();
            cancelAnimationFrame(frame);
            for (const motion of active.values()) motion.destroy();
            active.clear();
            for (const type of ['wheel', 'pointerdown', 'pointermove', 'touchmove'])
                root.removeEventListener(type, input, true);
            root.removeEventListener('keydown', keyboard, true);
        },
    };
}
