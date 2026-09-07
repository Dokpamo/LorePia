import type { Page } from './view-types';
import { horizontalWheel } from './horizontal-wheel';
import { setNavigationTiming } from './navigation-motion';

interface SwipeOptions {
    enabled: boolean;
    page: Page;
    subpage: boolean;
    navigate: (page: Page) => void;
    onsettle?: () => void;
}

interface Gesture {
    x: number;
    y: number;
    pointerId: number;
    width: number;
    scale: number;
    dragging: boolean;
    origin: number;
    mouseText: boolean;
    samples: { x: number; time: number }[];
}

export function swipePages(node: HTMLElement, initial: SwipeOptions) {
    let options = initial;
    let gesture: Gesture | null = null;
    let suppressClick = false;

    function cancel() {
        const previous = gesture;
        gesture = null;
        delete node.dataset.dragging;
        node.style.removeProperty('--ui-drag-offset');
        if (previous && node.hasPointerCapture(previous.pointerId))
            node.releasePointerCapture(previous.pointerId);
    }

    function down(event: PointerEvent) {
        cancel();
        suppressClick = false;
        if (!options.enabled || !event.isPrimary || event.button !== 0) return;
        if (event.target instanceof Element) {
            if (
                event.target.closest(
                    'input, textarea, select, a, label, [contenteditable], [data-ui-no-swipe]',
                )
            )
                return;
        }
        // Rightward mouse dragging in the phone frame mirrors touch regardless
        // of pace. Shift, double-click and leftward selection keep text native.
        const mouseText =
            event.pointerType === 'mouse' &&
            event.target instanceof Element &&
            !!event.target.closest(
                '.ui-message, .ui-speaker, .ui-card-description, .ui-card-details',
            );
        if (mouseText && (event.shiftKey || event.detail > 1)) return;
        const width = node.clientWidth;
        if (!width) return;
        const scale = node.getBoundingClientRect().width / width;
        if (!scale) return;
        gesture = {
            x: event.clientX,
            y: event.clientY,
            pointerId: event.pointerId,
            width,
            scale,
            dragging: false,
            origin: 0,
            mouseText,
            samples: [{ x: event.clientX, time: event.timeStamp }],
        };
    }

    function sample(current: Gesture, event: PointerEvent) {
        current.samples = current.samples.filter((point) => event.timeStamp - point.time <= 100);
        current.samples.push({ x: event.clientX, time: event.timeStamp });
    }

    function move(event: PointerEvent) {
        const current = gesture;
        if (event.pointerId !== current?.pointerId) return;
        const physicalDx = event.clientX - current.x;
        const dx = physicalDx / current.scale;
        const dy = event.clientY - current.y;
        if (!current.dragging) {
            if (Math.max(Math.abs(physicalDx), Math.abs(dy)) < 8) return;
            if (current.mouseText && (physicalDx < 0 || event.shiftKey)) {
                cancel();
                return;
            }
            if (Math.abs(physicalDx) <= Math.abs(dy) * 1.25) {
                cancel();
                return;
            }
            // Pick up an in-flight settle animation from its visible position.
            const parentLeft = node.parentElement?.getBoundingClientRect().left ?? 0;
            current.origin =
                (node.getBoundingClientRect().left - parentLeft) / current.scale +
                options.page * current.width;
            current.dragging = true;
            suppressClick = true;
            node.dataset.dragging = 'true';
            if (current.mouseText) window.getSelection()?.removeAllRanges();
            node.setPointerCapture(current.pointerId);
        }
        event.preventDefault();
        sample(current, event);
        const offset = dx + current.origin;
        const lastPage = options.subpage ? 2 : 1;
        const atEdge =
            (options.page === 0 && offset > 0) || (options.page === lastPage && offset < 0);
        const distance = atEdge
            ? (Math.abs(offset) * 0.22) / (1 + Math.abs(offset) / current.width)
            : Math.min(Math.abs(offset), current.width);
        node.style.setProperty('--ui-drag-offset', `${String(Math.sign(offset) * distance)}px`);
    }

    function up(event: PointerEvent) {
        const current = gesture;
        if (event.pointerId !== current?.pointerId) return;
        if (!current.dragging) {
            cancel();
            return;
        }
        sample(current, event);
        const first = current.samples[0];
        const elapsed = first ? event.timeStamp - first.time : 0;
        const velocity = first && elapsed > 0 ? (event.clientX - first.x) / elapsed : 0;
        const dx = (event.clientX - current.x) / current.scale + current.origin;
        const flick = Math.abs(velocity) >= 0.5 && Math.abs(dx) >= 24;
        const reversed = flick && Math.sign(velocity) !== Math.sign(dx);
        const threshold = Math.min(110, Math.max(64, current.width * 0.24));
        const next = options.page + (dx < 0 ? 1 : -1);
        const commit = !reversed && (Math.abs(dx) >= threshold || flick);
        setNavigationTiming(
            node,
            commit ? current.width - Math.abs(dx) : Math.abs(dx),
            Math.abs(velocity) / current.scale,
        );
        options.onsettle?.();
        cancel();
        if (commit && (next === 0 || next === 1 || (next === 2 && options.subpage)))
            options.navigate(next);
    }

    function pointerCancel(event: PointerEvent) {
        if (event.pointerId === gesture?.pointerId) cancel();
    }

    function click(event: MouseEvent) {
        if (!suppressClick || event.detail === 0) return;
        suppressClick = false;
        event.preventDefault();
        event.stopImmediatePropagation();
    }

    const wheel = horizontalWheel(node, {
        accepts: (event) =>
            options.enabled &&
            !(event.target instanceof Element && event.target.closest('[data-ui-no-swipe]')),
        move(distance) {
            const scale = node.getBoundingClientRect().width / node.clientWidth || 1;
            const dx = distance / scale;
            const last = options.subpage ? 2 : 1;
            const edge = (options.page === 0 && dx > 0) || (options.page === last && dx < 0);
            node.dataset.dragging = 'true';
            node.style.setProperty(
                '--ui-drag-offset',
                `${String(edge ? dx * 0.12 : Math.max(-node.clientWidth, Math.min(dx, node.clientWidth)))}px`,
            );
        },
        finish(distance) {
            setNavigationTiming(node, Math.max(0, node.clientWidth - Math.abs(distance)));
            options.onsettle?.();
            cancel();
            const next = options.page + (distance > 0 ? -1 : 1);
            if (
                Math.abs(distance) >= 64 &&
                (next === 0 || next === 1 || (next === 2 && options.subpage))
            )
                options.navigate(next);
        },
        cancel,
    });

    node.addEventListener('pointerdown', down);
    node.addEventListener('lostpointercapture', pointerCancel);
    node.addEventListener('click', click, true);
    window.addEventListener('pointermove', move, { passive: false });
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', pointerCancel);
    window.addEventListener('blur', cancel);
    window.addEventListener('resize', cancel);
    return {
        update(value: SwipeOptions) {
            if (
                value.enabled !== options.enabled ||
                value.page !== options.page ||
                value.subpage !== options.subpage
            ) {
                cancel();
                wheel.cancel();
            }
            options = value;
        },
        destroy() {
            cancel();
            wheel.destroy();
            node.removeEventListener('pointerdown', down);
            node.removeEventListener('lostpointercapture', pointerCancel);
            node.removeEventListener('click', click, true);
            window.removeEventListener('pointermove', move);
            window.removeEventListener('pointerup', up);
            window.removeEventListener('pointercancel', pointerCancel);
            window.removeEventListener('blur', cancel);
            window.removeEventListener('resize', cancel);
        },
    };
}
