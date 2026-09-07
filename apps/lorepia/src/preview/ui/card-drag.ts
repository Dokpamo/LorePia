interface DragOptions {
    ondrop: (source: string, target: string) => void;
}

/** Mouse drag or touch hold, isolated from the page's horizontal navigation. */
export function cardDrag(node: HTMLElement, initial: DragOptions) {
    let options = initial;
    let pending: { id: number; x: number; y: number; button: HTMLElement; touch: boolean } | null =
        null;
    let ghost: HTMLElement | null = null;
    let target: HTMLElement | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let suppress = false;
    let scrolling = false;
    let frame: number | undefined;
    let point = { x: 0, y: 0 };

    function clear() {
        clearTimeout(timer);
        timer = undefined;
        if (frame !== undefined) cancelAnimationFrame(frame);
        frame = undefined;
        ghost?.remove();
        ghost = null;
        target?.removeAttribute('data-drop-target');
        target = null;
        const current = pending;
        pending = null;
        current?.button.removeAttribute('data-card-dragging');
        delete node.dataset.organizing;
        scrolling = false;
        if (current && node.hasPointerCapture(current.id)) node.releasePointerCapture(current.id);
    }
    function start() {
        if (!pending) return;
        suppress = true;
        pending.button.dataset.cardDragging = 'true';
        node.dataset.organizing = 'true';
        ghost = pending.button.cloneNode(true) as HTMLElement;
        ghost.removeAttribute('data-rail-id');
        ghost.removeAttribute('aria-label');
        ghost.setAttribute('aria-hidden', 'true');
        ghost.setAttribute('inert', '');
        ghost.className = 'ui-card-drag-ghost';
        node.closest('.ui-preview')?.append(ghost);
        node.setPointerCapture(pending.id);
        paint();
    }
    function paint() {
        frame = undefined;
        if (!pending || !ghost) return;
        const root = node.closest<HTMLElement>('.ui-preview');
        if (!root) return;
        const bounds = root.getBoundingClientRect();
        const scale = bounds.width / (root.clientWidth || bounds.width);
        ghost.style.transform = `translate3d(${String((point.x - bounds.left) / scale - 24)}px,${String((point.y - bounds.top) / scale - 24)}px,0) scale(1.08)`;
        const next =
            Array.from(node.querySelectorAll<HTMLElement>('[data-rail-id]')).find((item) => {
                if (item === pending?.button) return false;
                const rect = item.getBoundingClientRect();
                return (
                    point.x >= rect.left &&
                    point.x <= rect.right &&
                    point.y >= rect.top &&
                    point.y <= rect.bottom
                );
            }) ?? null;
        if (next !== target) {
            target?.removeAttribute('data-drop-target');
            target = next;
            if (target) target.dataset.dropTarget = 'true';
        }
        const boundsList = node.getBoundingClientRect();
        if (point.y > boundsList.bottom - 24) node.scrollTop += 6;
        else if (point.y < boundsList.top + 24) node.scrollTop -= 6;
        if (point.y > boundsList.bottom - 24 || point.y < boundsList.top + 24)
            frame = requestAnimationFrame(paint);
    }
    function down(event: PointerEvent) {
        clear();
        suppress = false;
        if (!event.isPrimary || event.button !== 0 || !(event.target instanceof Element)) return;
        const button = event.target.closest<HTMLElement>('[data-rail-id]');
        if (!button || !node.contains(button)) return;
        pending = {
            id: event.pointerId,
            x: event.clientX,
            y: event.clientY,
            button,
            touch: event.pointerType !== 'mouse',
        };
        point = { x: event.clientX, y: event.clientY };
        if (pending.touch) timer = setTimeout(start, 300);
    }
    function move(event: PointerEvent) {
        if (event.pointerId !== pending?.id) return;
        const previousY = point.y;
        point = { x: event.clientX, y: event.clientY };
        if (scrolling) {
            node.scrollTop += previousY - point.y;
            event.preventDefault();
            return;
        }
        if (!ghost) {
            if (Math.hypot(event.clientX - pending.x, event.clientY - pending.y) < 6) return;
            if (pending.touch) {
                clearTimeout(timer);
                timer = undefined;
                scrolling = true;
                suppress = true;
                node.scrollTop += previousY - point.y;
                event.preventDefault();
                return;
            }
            start();
        }
        event.preventDefault();
        frame ??= requestAnimationFrame(paint);
    }
    function up(event: PointerEvent) {
        if (event.pointerId !== pending?.id) return;
        point = { x: event.clientX, y: event.clientY };
        if (frame !== undefined) cancelAnimationFrame(frame);
        frame = undefined;
        if (ghost) paint();
        const sourceId = pending.button.dataset.railId;
        const targetId = target?.dataset.railId;
        clear();
        if (sourceId && targetId) options.ondrop(sourceId, targetId);
    }
    function cancel(event: PointerEvent) {
        if (event.pointerId === pending?.id) clear();
    }
    function click(event: MouseEvent) {
        if (!suppress || event.detail === 0) return;
        suppress = false;
        event.preventDefault();
        event.stopImmediatePropagation();
    }
    function context(event: MouseEvent) {
        if (ghost) {
            event.preventDefault();
            event.stopImmediatePropagation();
        }
    }
    node.addEventListener('pointerdown', down);
    node.addEventListener('click', click, true);
    node.addEventListener('contextmenu', context, true);
    node.addEventListener('lostpointercapture', cancel);
    window.addEventListener('pointermove', move, { passive: false });
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', cancel);
    window.addEventListener('blur', clear);
    window.addEventListener('resize', clear);
    return {
        update(next: DragOptions) {
            options = next;
        },
        destroy() {
            clear();
            node.removeEventListener('pointerdown', down);
            node.removeEventListener('click', click, true);
            node.removeEventListener('contextmenu', context, true);
            node.removeEventListener('lostpointercapture', cancel);
            window.removeEventListener('pointermove', move);
            window.removeEventListener('pointerup', up);
            window.removeEventListener('pointercancel', cancel);
            window.removeEventListener('blur', clear);
            window.removeEventListener('resize', clear);
        },
    };
}
