import { horizontalWheel } from '../workspace/horizontal-wheel';

interface RootSwipeOptions {
    enabled: boolean;
    index: number;
    count: number;
    prepare: (index: number) => void;
    select: (index: number) => void;
}
interface Pointer {
    id: number;
    x: number;
    y: number;
    width: number;
    origin: number;
    dragging: boolean;
    samples: { x: number; time: number }[];
}

/** Root destinations follow a horizontal pan without taking over scrolling or nested pages. */
export function rootTabSwipe(node: HTMLElement, initial: RootSwipeOptions) {
    let options = initial;
    let pointer: Pointer | null = null;
    let suppressClick = false;
    let settlingTo: number | undefined;
    let timeout: ReturnType<typeof setTimeout> | undefined;
    let wheelOrigin: number | undefined;
    let prepared: number | undefined;

    function release() {
        const previous = pointer;
        pointer = null;
        if (previous && node.hasPointerCapture(previous.id))
            node.releasePointerCapture(previous.id);
    }
    function cancel() {
        release();
        clearTimeout(timeout);
        settlingTo = undefined;
        wheelOrigin = undefined;
        prepared = undefined;
        delete node.dataset.rootDragging;
        delete node.dataset.rootSettling;
        node.style.removeProperty('--seed-tab-drag');
    }
    function accepts(target: EventTarget | null) {
        if (!options.enabled || node.closest('[inert], [hidden]')) return false;
        if (!(target instanceof Element)) return true;
        if (
            target.closest(
                'input, textarea, select, a, label, [contenteditable], [role="slider"], [data-ui-no-swipe], [data-ui-selectable], [data-image-control]',
            )
        )
            return false;
        for (
            let child: Element | null = target;
            child && child !== node;
            child = child.parentElement
        ) {
            if (
                child.scrollWidth > child.clientWidth + 1 &&
                /auto|scroll/.test(getComputedStyle(child).overflowX)
            )
                return false;
        }
        return true;
    }
    function pickUp() {
        const active = node.querySelector<HTMLElement>(
            ':scope > .seed-root[data-root-tab]:not([aria-hidden="true"])',
        );
        const offset = active
            ? active.getBoundingClientRect().left - node.getBoundingClientRect().left
            : 0;
        clearTimeout(timeout);
        settlingTo = undefined;
        delete node.dataset.rootSettling;
        node.dataset.rootDragging = 'true';
        node.style.setProperty('--seed-tab-drag', `${String(offset)}px`);
        return offset;
    }
    function paint(distance: number, width: number) {
        const next = options.index + (distance < 0 ? 1 : -1);
        const edge = next < 0 || next >= options.count;
        if (!edge && prepared !== next) {
            prepared = next;
            options.prepare(next);
        }
        const amount = edge
            ? (distance * 0.18) / (1 + Math.abs(distance) / width)
            : Math.max(-width, Math.min(width, distance));
        node.style.setProperty('--seed-tab-drag', `${String(amount)}px`);
    }
    function settle(next = options.index) {
        release();
        wheelOrigin = undefined;
        prepared = undefined;
        settlingTo = next;
        delete node.dataset.rootDragging;
        node.dataset.rootSettling = 'true';
        node.style.setProperty('--seed-tab-drag', '0px');
        if (next !== options.index) options.select(next);
        clearTimeout(timeout);
        timeout = setTimeout(
            () => {
                settlingTo = undefined;
                delete node.dataset.rootSettling;
            },
            matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 300,
        );
    }
    function finish(distance: number, velocity: number, width: number) {
        const next = options.index + (distance < 0 ? 1 : -1);
        const flick = Math.abs(velocity) >= 0.5 && Math.abs(distance) >= 24;
        const reversed = flick && Math.sign(velocity) !== Math.sign(distance);
        const threshold = Math.min(110, Math.max(64, width * 0.24));
        settle(
            !reversed &&
                (Math.abs(distance) >= threshold || flick) &&
                next >= 0 &&
                next < options.count
                ? next
                : options.index,
        );
    }
    function down(event: PointerEvent) {
        if (pointer?.dragging) settle();
        else release();
        suppressClick = false;
        if (!event.isPrimary || event.button !== 0 || !accepts(event.target) || !node.clientWidth)
            return;
        pointer = {
            id: event.pointerId,
            x: event.clientX,
            y: event.clientY,
            width: node.clientWidth,
            origin: 0,
            dragging: false,
            samples: [{ x: event.clientX, time: event.timeStamp }],
        };
    }
    function sample(current: Pointer, event: PointerEvent) {
        current.samples = current.samples.filter((item) => event.timeStamp - item.time <= 100);
        current.samples.push({ x: event.clientX, time: event.timeStamp });
    }
    function move(event: PointerEvent) {
        const current = pointer;
        if (event.pointerId !== current?.id) return;
        const x = event.clientX - current.x;
        const y = event.clientY - current.y;
        if (!current.dragging) {
            if (Math.max(Math.abs(x), Math.abs(y)) < 8) return;
            if (Math.abs(x) <= Math.abs(y) * 1.25) {
                release();
                return;
            }
            current.origin = pickUp();
            current.dragging = true;
            suppressClick = true;
            node.setPointerCapture(current.id);
            window.getSelection()?.removeAllRanges();
        }
        event.preventDefault();
        sample(current, event);
        paint(x + current.origin, current.width);
    }
    function up(event: PointerEvent) {
        const current = pointer;
        if (event.pointerId !== current?.id) return;
        if (!current.dragging) {
            release();
            return;
        }
        sample(current, event);
        const first = current.samples[0];
        const elapsed = first ? event.timeStamp - first.time : 0;
        const velocity = first && elapsed > 0 ? (event.clientX - first.x) / elapsed : 0;
        finish(event.clientX - current.x + current.origin, velocity, current.width);
    }
    function pointerCancel(event: PointerEvent) {
        if (event.pointerId === pointer?.id) settle();
    }
    function click(event: MouseEvent) {
        if (!suppressClick || event.detail === 0) return;
        suppressClick = false;
        event.preventDefault();
        event.stopImmediatePropagation();
    }
    function nativeDrag(event: DragEvent) {
        if (pointer) event.preventDefault();
    }
    const wheel = horizontalWheel(node, {
        accepts: (event) => !pointer && accepts(event.target),
        move(distance) {
            wheelOrigin ??= pickUp();
            paint(distance + wheelOrigin, node.clientWidth);
        },
        finish: (distance) => finish(distance + (wheelOrigin ?? 0), 0, node.clientWidth),
        cancel,
    });
    node.addEventListener('pointerdown', down);
    node.addEventListener('lostpointercapture', pointerCancel);
    node.addEventListener('dragstart', nativeDrag);
    node.addEventListener('click', click, true);
    window.addEventListener('pointermove', move, { passive: false });
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', pointerCancel);
    window.addEventListener('resize', cancel);
    window.addEventListener('blur', cancel);
    return {
        update(next: RootSwipeOptions) {
            const changed =
                next.enabled !== options.enabled ||
                (next.index !== options.index && next.index !== settlingTo);
            options = next;
            if (changed) {
                cancel();
                wheel.cancel();
            }
        },
        destroy() {
            wheel.destroy();
            cancel();
            node.removeEventListener('pointerdown', down);
            node.removeEventListener('lostpointercapture', pointerCancel);
            node.removeEventListener('dragstart', nativeDrag);
            node.removeEventListener('click', click, true);
            window.removeEventListener('pointermove', move);
            window.removeEventListener('pointerup', up);
            window.removeEventListener('pointercancel', pointerCancel);
            window.removeEventListener('resize', cancel);
            window.removeEventListener('blur', cancel);
        },
    };
}
