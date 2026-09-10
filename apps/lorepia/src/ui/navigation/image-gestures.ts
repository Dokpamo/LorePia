import { horizontalWheel } from '../workspace/horizontal-wheel';

export interface ImageGestureOptions {
    next: () => void;
    previous: () => void;
    dismiss?: () => void;
    move: (x: number, y: number, active: boolean) => void;
}

/** Photo interactions own horizontal pans; only a downward pan dismisses a viewer. */
export function imageGestures(node: HTMLElement, options: ImageGestureOptions) {
    let pointer: { id: number; x: number; y: number; time: number; axis: 'x' | 'y' | null } | null =
        null;
    let suppressClick = false;
    function reset() {
        pointer = null;
        options.move(0, 0, false);
    }
    function down(event: PointerEvent) {
        if (
            !event.isPrimary ||
            event.button !== 0 ||
            (event.target instanceof Element && event.target.closest('[data-image-control]'))
        )
            return;
        event.stopPropagation();
        pointer = {
            id: event.pointerId,
            x: event.clientX,
            y: event.clientY,
            time: event.timeStamp,
            axis: null,
        };
        suppressClick = false;
    }
    function move(event: PointerEvent) {
        if (pointer?.id !== event.pointerId) return;
        const x = event.clientX - pointer.x;
        const y = event.clientY - pointer.y;
        if (!pointer.axis && Math.max(Math.abs(x), Math.abs(y)) > 10) {
            pointer.axis = Math.abs(x) > Math.abs(y) * 1.15 ? 'x' : 'y';
            if (pointer.axis === 'y' && !options.dismiss) {
                reset();
                return;
            }
            node.setPointerCapture(event.pointerId);
        }
        if (!pointer.axis) return;
        event.preventDefault();
        event.stopPropagation();
        suppressClick = true;
        options.move(pointer.axis === 'x' ? x : 0, pointer.axis === 'y' ? Math.max(0, y) : 0, true);
    }
    function up(event: PointerEvent) {
        if (pointer?.id !== event.pointerId) return;
        const { axis, x, y, time } = pointer;
        const dx = event.clientX - x;
        const dy = event.clientY - y;
        const elapsed = Math.max(1, event.timeStamp - time);
        if (node.hasPointerCapture(event.pointerId)) node.releasePointerCapture(event.pointerId);
        reset();
        if (
            axis === 'x' &&
            (Math.abs(dx) > Math.min(90, node.clientWidth * 0.18) ||
                (Math.abs(dx) > 24 && Math.abs(dx) / elapsed > 0.5))
        ) {
            if (dx < 0) options.next();
            else options.previous();
        } else if (axis === 'y' && (dy > 100 || (dy > 30 && dy / elapsed > 0.55)))
            options.dismiss?.();
    }
    function click(event: MouseEvent) {
        if (!suppressClick) return;
        suppressClick = false;
        event.preventDefault();
        event.stopImmediatePropagation();
    }
    const wheel = horizontalWheel(node, {
        accepts: () => true,
        move: (x) => {
            suppressClick = true;
            options.move(x, 0, true);
        },
        finish: (x) => {
            reset();
            if (x < -60) options.next();
            else if (x > 60) options.previous();
        },
        cancel: reset,
    });
    node.addEventListener('pointerdown', down);
    node.addEventListener('pointermove', move);
    node.addEventListener('pointerup', up);
    node.addEventListener('pointercancel', reset);
    node.addEventListener('click', click, true);
    return {
        update(next: ImageGestureOptions) {
            options = next;
        },
        destroy() {
            wheel.destroy();
            node.removeEventListener('pointerdown', down);
            node.removeEventListener('pointermove', move);
            node.removeEventListener('pointerup', up);
            node.removeEventListener('pointercancel', reset);
            node.removeEventListener('click', click, true);
        },
    };
}
