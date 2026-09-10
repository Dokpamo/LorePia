/** Long press reserves message tools; ordinary drag/selection remains native. */
export function messagePress(node: HTMLElement, activate: () => void) {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let pointer: { id: number; x: number; y: number } | undefined;
    let suppressClick = false;
    const interactive = (event: Event) =>
        event.target instanceof Element &&
        !!event.target.closest(
            'a, button, input, textarea, select, [contenteditable], [data-ui-interactive]',
        );
    function cancel() {
        clearTimeout(timer);
        timer = undefined;
        pointer = undefined;
    }
    function down(event: PointerEvent) {
        cancel();
        suppressClick = false;
        if (event.button !== 0 || !event.isPrimary || interactive(event)) return;
        pointer = { id: event.pointerId, x: event.clientX, y: event.clientY };
        timer = setTimeout(() => {
            timer = undefined;
            if (!node.isConnected || node.closest('[inert]')) return;
            suppressClick = true;
            activate();
        }, 450);
    }
    function move(event: PointerEvent) {
        if (
            pointer?.id === event.pointerId &&
            Math.hypot(event.clientX - pointer.x, event.clientY - pointer.y) > 10
        )
            cancel();
    }
    function context(event: MouseEvent) {
        if (interactive(event)) return;
        event.preventDefault();
        cancel();
        if (!suppressClick) activate();
        suppressClick = true;
    }
    function click(event: MouseEvent) {
        if (suppressClick && event.detail !== 0) {
            event.preventDefault();
            event.stopImmediatePropagation();
        }
        suppressClick = false;
    }
    node.addEventListener('pointerdown', down);
    node.addEventListener('contextmenu', context);
    node.addEventListener('click', click, true);
    window.addEventListener('pointermove', move, true);
    window.addEventListener('pointerup', cancel, true);
    window.addEventListener('pointercancel', cancel, true);
    window.addEventListener('blur', cancel);
    window.addEventListener('scroll', cancel, true);
    return {
        update(next: () => void) {
            activate = next;
        },
        destroy() {
            cancel();
            node.removeEventListener('pointerdown', down);
            node.removeEventListener('contextmenu', context);
            node.removeEventListener('click', click, true);
            window.removeEventListener('pointermove', move, true);
            window.removeEventListener('pointerup', cancel, true);
            window.removeEventListener('pointercancel', cancel, true);
            window.removeEventListener('blur', cancel);
            window.removeEventListener('scroll', cancel, true);
        },
    };
}
