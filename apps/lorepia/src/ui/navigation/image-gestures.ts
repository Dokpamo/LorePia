import { horizontalWheel } from '../workspace/horizontal-wheel';

export interface ImageGestureOptions {
    next: () => void;
    previous: () => void;
    canNext?: boolean;
    canPrevious?: boolean;
    backAtStart?: boolean;
    dismiss?: () => void;
    move: (x: number, y: number, active: boolean) => void;
}

/** Photo interactions own horizontal pans; only a downward pan dismisses a viewer. */
export function imageGestures(node: HTMLElement, options: ImageGestureOptions) {
    let pointer: {
        id: number;
        x: number;
        y: number;
        time: number;
        axis: 'x' | 'y' | null;
        back: boolean;
        scale: number;
    } | null = null;
    let suppressClick = false;
    let wheelBack: boolean | undefined;
    let wheelTimer: ReturnType<typeof setTimeout> | undefined;
    function reset() {
        const id = pointer?.id;
        pointer = null;
        if (id !== undefined && node.hasPointerCapture(id)) node.releasePointerCapture(id);
        options.move(0, 0, false);
    }
    function horizontal(distance: number) {
        const edge = distance < 0 ? options.canNext === false : options.canPrevious === false;
        const width = node.clientWidth || 1;
        // The same resistance as the root tabs, with the current photo behind the edge.
        return edge
            ? (distance * 0.18) / (1 + Math.abs(distance) / width)
            : Math.max(-width, Math.min(width, distance));
    }
    function finish(distance: number) {
        if (distance < 0 && options.canNext !== false) options.next();
        else if (distance > 0 && options.canPrevious !== false) options.previous();
    }
    function down(event: PointerEvent) {
        if (
            !event.isPrimary ||
            event.button !== 0 ||
            (event.target instanceof Element && event.target.closest('[data-image-control]'))
        )
            return;
        reset();
        const back = options.backAtStart === true && options.canPrevious === false;
        // Register the same down event with the parent, so a rightward pan can
        // move the whole page immediately instead of firing a late back click.
        if (!back) event.stopPropagation();
        else event.preventDefault(); // Keep WebKit's native image tracking out of the handoff.
        pointer = {
            id: event.pointerId,
            x: event.clientX,
            y: event.clientY,
            time: event.timeStamp,
            axis: null,
            back,
            scale: node.getBoundingClientRect().width / (node.clientWidth || 1) || 1,
        };
        suppressClick = false;
    }
    function move(event: PointerEvent) {
        if (pointer?.id !== event.pointerId) return;
        const x = event.clientX - pointer.x;
        const y = event.clientY - pointer.y;
        if (!pointer.axis && Math.max(Math.abs(x), Math.abs(y)) > 10) {
            pointer.axis = Math.abs(x) > Math.abs(y) * 1.15 ? 'x' : 'y';
            if (pointer.axis === 'x' && x > 0 && pointer.back) {
                event.preventDefault();
                reset();
                return;
            }
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
        options.move(
            pointer.axis === 'x' ? horizontal(x / pointer.scale) : 0,
            pointer.axis === 'y' ? Math.max(0, y / pointer.scale) : 0,
            true,
        );
    }
    function up(event: PointerEvent) {
        if (pointer?.id !== event.pointerId) return;
        if (pointer.axis) event.stopPropagation();
        const { axis, x, y, time } = pointer;
        const dx = event.clientX - x;
        const dy = event.clientY - y;
        const elapsed = Math.max(1, event.timeStamp - time);
        reset();
        if (
            axis === 'x' &&
            (Math.abs(dx) > Math.min(90, node.clientWidth * 0.18) ||
                (Math.abs(dx) > 24 && Math.abs(dx) / elapsed > 0.5))
        ) {
            finish(dx);
        } else if (axis === 'y' && (dy > 100 || (dy > 30 && dy / elapsed > 0.55)))
            options.dismiss?.();
    }
    function click(event: MouseEvent) {
        if (!suppressClick) return;
        suppressClick = false;
        event.preventDefault();
        event.stopImmediatePropagation();
    }
    function cancel(event: PointerEvent) {
        if (pointer?.id === event.pointerId) reset();
    }
    function nativeDrag(event: DragEvent) {
        if (pointer) event.preventDefault();
    }
    function resetWheel() {
        clearTimeout(wheelTimer);
        wheelBack = undefined;
    }
    const wheel = horizontalWheel(node, {
        accepts: (event) => {
            if (event.deltaX === 0 && event.deltaY === 0) return false;
            if (wheelBack === undefined && Math.abs(event.deltaX) > Math.abs(event.deltaY))
                wheelBack =
                    options.backAtStart === true &&
                    options.canPrevious === false &&
                    event.deltaX < 0;
            clearTimeout(wheelTimer);
            wheelTimer = setTimeout(resetWheel, 180);
            return wheelBack !== true;
        },
        move: (x) => {
            suppressClick = true;
            const scale = node.getBoundingClientRect().width / (node.clientWidth || 1) || 1;
            options.move(horizontal(x / scale), 0, true);
        },
        finish: (x) => {
            reset();
            if (Math.abs(x) > 60) finish(x);
        },
        cancel: reset,
    });
    node.addEventListener('pointerdown', down);
    node.addEventListener('pointermove', move);
    node.addEventListener('pointerup', up);
    node.addEventListener('pointercancel', cancel);
    node.addEventListener('lostpointercapture', cancel);
    node.addEventListener('dragstart', nativeDrag);
    node.addEventListener('click', click, true);
    return {
        update(next: ImageGestureOptions) {
            options = next;
        },
        destroy() {
            wheel.destroy();
            resetWheel();
            reset();
            node.removeEventListener('pointerdown', down);
            node.removeEventListener('pointermove', move);
            node.removeEventListener('pointerup', up);
            node.removeEventListener('pointercancel', cancel);
            node.removeEventListener('lostpointercapture', cancel);
            node.removeEventListener('dragstart', nativeDrag);
            node.removeEventListener('click', click, true);
        },
    };
}
