interface MenuOptions {
    anchor: HTMLElement;
    onclose: (restoreFocus: boolean) => void;
    view: string;
}

/** SEED Menu: 8px gutter, flip above when below is full, then clamp to the viewport. */
export function positionMessageMenu(node: HTMLElement, initial: MenuOptions) {
    let options = initial;
    let disposed = false;
    let frame: number | undefined;
    const popover = typeof node.showPopover === 'function';
    function position() {
        frame = undefined;
        if (disposed) return;
        const { anchor } = options;
        if (!anchor.isConnected || anchor.closest('[inert], [hidden]')) {
            options.onclose(false);
            return;
        }
        const viewport = window.visualViewport;
        const left = viewport?.offsetLeft ?? 0;
        const top = viewport?.offsetTop ?? 0;
        const width = viewport?.width ?? window.innerWidth;
        const height = viewport?.height ?? window.innerHeight;
        const rect = anchor.getBoundingClientRect();
        const footer = anchor
            .closest('.ui-chat')
            ?.querySelector('.ui-compose-field')
            ?.getBoundingClientRect();
        const bottom = Math.min(top + height, footer?.top ?? Infinity) - 8;
        const menuWidth = Math.min(240, width - 16);
        node.style.width = `${String(menuWidth)}px`;
        node.style.maxHeight = `${String(Math.max(80, Math.min(480, bottom - top - 8)))}px`;
        const menuHeight = node.offsetHeight;
        const above = rect.bottom + 8 + menuHeight > bottom && rect.top - 8 - menuHeight >= top + 8;
        const y = Math.max(
            top + 8,
            Math.min(above ? rect.top - 8 - menuHeight : rect.bottom + 8, bottom - menuHeight),
        );
        const preferred = anchor.closest('.ui-user-turn') ? rect.right - menuWidth : rect.left;
        const x = Math.max(left + 8, Math.min(preferred, left + width - menuWidth - 8));
        node.style.left = `${String(x)}px`;
        node.style.top = `${String(y)}px`;
        node.style.transformOrigin = `${String(Math.max(12, Math.min(menuWidth - 12, rect.left + rect.width / 2 - x)))}px ${above ? 'bottom' : 'top'}`;
        node.dataset.positioned = 'true';
    }
    function schedule() {
        frame ??= requestAnimationFrame(position);
    }
    function outside(event: PointerEvent) {
        if (
            event.target instanceof Node &&
            !node.contains(event.target) &&
            !options.anchor.contains(event.target)
        )
            options.onclose(false);
    }
    function scrolled(event: Event) {
        if (event.target instanceof Node && node.contains(event.target)) return;
        options.onclose(false);
    }
    function toggled(event: Event) {
        if (!disposed && 'newState' in event && event.newState === 'closed') options.onclose(true);
    }
    if (popover) {
        node.setAttribute('popover', 'manual');
        node.showPopover();
        node.addEventListener('toggle', toggled);
    }
    position();
    node.querySelector<HTMLButtonElement>(
        '[data-menu-initial], [role="menuitem"]:not(:disabled)',
    )?.focus({ preventScroll: true });
    window.addEventListener('pointerdown', outside, true);
    options.anchor.closest('.ui-messages')?.addEventListener('scroll', scrolled);
    window.addEventListener('resize', schedule);
    window.visualViewport?.addEventListener('resize', schedule);
    return {
        update(next: MenuOptions) {
            const changed = options.view !== next.view;
            options = next;
            schedule();
            if (changed)
                queueMicrotask(() => {
                    if (!disposed)
                        node.querySelector<HTMLButtonElement>(
                            '[data-menu-initial], [role="menuitem"]:not(:disabled)',
                        )?.focus({ preventScroll: true });
                });
        },
        destroy() {
            disposed = true;
            if (frame !== undefined) cancelAnimationFrame(frame);
            node.removeEventListener('toggle', toggled);
            if (popover && node.matches(':popover-open')) node.hidePopover();
            window.removeEventListener('pointerdown', outside, true);
            options.anchor.closest('.ui-messages')?.removeEventListener('scroll', scrolled);
            window.removeEventListener('resize', schedule);
            window.visualViewport?.removeEventListener('resize', schedule);
        },
    };
}
