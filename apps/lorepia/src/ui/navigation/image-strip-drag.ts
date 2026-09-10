interface StripPointer {
    id: number;
    x: number;
    y: number;
    origin: number;
    scale: number;
    dragging: boolean;
    lastScroll: number;
    lastTime: number;
    velocity: number;
}

/** Mouse dragging supplements the strip's native touch and trackpad scrolling. */
export function imageStripDrag(
    node: HTMLElement,
    oninteract: () => void,
    onscroll?: () => void,
    onsettle?: () => void,
) {
    let pointer: StripPointer | null = null;
    let frame = 0;
    let suppressClick = false;

    function cancel() {
        cancelAnimationFrame(frame);
        frame = 0;
        const previous = pointer;
        pointer = null;
        delete node.dataset.stripDragging;
        if (previous && node.hasPointerCapture(previous.id))
            node.releasePointerCapture(previous.id);
    }
    function interrupt() {
        cancel();
        oninteract();
    }
    function scroll(to: number) {
        node.scrollLeft = Math.max(0, Math.min(node.scrollWidth - node.clientWidth, to));
        onscroll?.();
    }
    function coast(initialVelocity: number) {
        if (matchMedia('(prefers-reduced-motion: reduce)').matches) {
            onsettle?.();
            return;
        }
        let velocity = Math.max(-2.4, Math.min(2.4, initialVelocity));
        let last = performance.now();
        function step(now: number) {
            const elapsed = Math.min(32, now - last);
            const decay = Math.exp(-elapsed / 180);
            const from = node.scrollLeft;
            scroll(from + velocity * 180 * (1 - decay));
            velocity *= decay;
            last = now;
            if (Math.abs(velocity) > 0.02 && (elapsed === 0 || node.scrollLeft !== from))
                frame = requestAnimationFrame(step);
            else {
                frame = 0;
                onsettle?.();
            }
        }
        if (Math.abs(velocity) > 0.02) frame = requestAnimationFrame(step);
        else onsettle?.();
    }
    function down(event: PointerEvent) {
        interrupt();
        suppressClick = false;
        if (
            !event.isPrimary ||
            event.button !== 0 ||
            event.pointerType !== 'mouse' ||
            node.scrollWidth <= node.clientWidth
        )
            return;
        pointer = {
            id: event.pointerId,
            x: event.clientX,
            y: event.clientY,
            origin: node.scrollLeft,
            scale: node.getBoundingClientRect().width / node.offsetWidth || 1,
            dragging: false,
            lastScroll: node.scrollLeft,
            lastTime: event.timeStamp,
            velocity: 0,
        };
    }
    function move(event: PointerEvent) {
        const current = pointer;
        if (current?.id !== event.pointerId) return;
        const dx = event.clientX - current.x;
        const dy = event.clientY - current.y;
        if (!current.dragging) {
            if (Math.max(Math.abs(dx), Math.abs(dy)) < 6) return;
            if (Math.abs(dx) <= Math.abs(dy) * 1.15) {
                cancel();
                return;
            }
            current.dragging = true;
            node.setPointerCapture(current.id);
            node.dataset.stripDragging = 'true';
        }
        event.preventDefault();
        suppressClick = true;
        scroll(current.origin - dx / current.scale);
        const elapsed = event.timeStamp - current.lastTime;
        if (elapsed > 0) current.velocity = (node.scrollLeft - current.lastScroll) / elapsed;
        current.lastScroll = node.scrollLeft;
        current.lastTime = event.timeStamp;
    }
    function up(event: PointerEvent) {
        const current = pointer;
        if (current?.id !== event.pointerId) return;
        cancel();
        if (current.dragging && event.timeStamp - current.lastTime < 100) coast(current.velocity);
        else if (current.dragging) onsettle?.();
    }
    function lost(event: PointerEvent) {
        if (pointer?.id === event.pointerId) {
            cancel();
            onsettle?.();
        }
    }
    function click(event: MouseEvent) {
        if (suppressClick && event.detail !== 0) {
            suppressClick = false;
            event.preventDefault();
            event.stopImmediatePropagation();
        } else {
            suppressClick = false;
            interrupt();
        }
    }
    function nativeDrag(event: DragEvent) {
        event.preventDefault();
    }

    node.addEventListener('pointerdown', down);
    node.addEventListener('click', click, true);
    node.addEventListener('wheel', interrupt, { passive: true });
    node.addEventListener('keydown', interrupt, true);
    node.addEventListener('dragstart', nativeDrag);
    node.addEventListener('lostpointercapture', lost);
    window.addEventListener('pointermove', move, { passive: false });
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', lost);
    window.addEventListener('blur', cancel);
    window.addEventListener('resize', cancel);
    return {
        get active() {
            return pointer?.dragging === true || frame !== 0;
        },
        cancel,
        destroy() {
            cancel();
            node.removeEventListener('pointerdown', down);
            node.removeEventListener('click', click, true);
            node.removeEventListener('wheel', interrupt);
            node.removeEventListener('keydown', interrupt, true);
            node.removeEventListener('dragstart', nativeDrag);
            node.removeEventListener('lostpointercapture', lost);
            window.removeEventListener('pointermove', move);
            window.removeEventListener('pointerup', up);
            window.removeEventListener('pointercancel', lost);
            window.removeEventListener('blur', cancel);
            window.removeEventListener('resize', cancel);
        },
    };
}
