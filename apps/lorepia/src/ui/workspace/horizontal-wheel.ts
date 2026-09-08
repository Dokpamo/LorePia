interface WheelOptions {
    accepts: (event: WheelEvent) => boolean;
    move: (distance: number) => void;
    finish: (distance: number) => void;
    cancel: () => void;
}

/** Trackpad deltas move the surface directly; a quiet interval ends one gesture. */
export function horizontalWheel(node: HTMLElement, options: WheelOptions) {
    let distance = 0;
    let vertical = 0;
    let active = false;
    let blocked = false;
    let timeout: ReturnType<typeof setTimeout> | undefined;

    function reset() {
        if (timeout !== undefined) clearTimeout(timeout);
        timeout = undefined;
        distance = 0;
        vertical = 0;
        active = false;
        blocked = false;
    }
    function cancel() {
        const wasActive = active;
        reset();
        if (wasActive) options.cancel();
    }
    function quiet() {
        if (timeout !== undefined) clearTimeout(timeout);
        timeout = setTimeout(() => {
            const end = distance;
            const shouldFinish = active;
            reset();
            if (shouldFinish) options.finish(end);
        }, 160);
    }
    function wheel(event: WheelEvent) {
        if (event.defaultPrevented || !options.accepts(event)) return;
        if (event.ctrlKey) {
            cancel();
            return;
        }
        const x = event.deltaX;
        const y = event.deltaY;
        // AppKit can start/end a trackpad phase with a zero delta. Tiny initial
        // diagonal noise must not lock out the user's subsequent horizontal pan.
        if (x === 0 && y === 0) return;
        const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? node.clientWidth : 1;
        distance -= x * unit;
        vertical += y * unit;
        if (!active && Math.max(Math.abs(distance), Math.abs(vertical)) >= 8) {
            if (Math.abs(vertical) > Math.abs(distance) * 1.2) blocked = true;
            else if (!blocked && Math.abs(distance) > Math.abs(vertical) * 1.3) active = true;
        }
        if (active) {
            event.preventDefault();
            options.move(distance);
        }
        quiet();
    }
    node.addEventListener('wheel', wheel, { passive: false });
    node.addEventListener('pointerdown', cancel);
    window.addEventListener('resize', cancel);
    window.addEventListener('blur', cancel);
    return {
        cancel,
        destroy() {
            cancel();
            node.removeEventListener('wheel', wheel);
            node.removeEventListener('pointerdown', cancel);
            window.removeEventListener('resize', cancel);
            window.removeEventListener('blur', cancel);
        },
    };
}
