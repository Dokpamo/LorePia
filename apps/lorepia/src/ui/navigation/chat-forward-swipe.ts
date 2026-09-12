import { horizontalWheel } from '../workspace/horizontal-wheel';
import { tick } from 'svelte';

interface Options {
    enabled: boolean;
    target: () => HTMLElement | undefined;
    onopen: () => void;
}
interface Pointer {
    id: number;
    x: number;
    y: number;
    width: number;
    scale: number;
    dragging: boolean;
    lastX: number;
    time: number;
    velocity: number;
}

/** The right page follows a leftward pan; rightward navigation keeps edgeBack as its owner. */
export function chatForwardSwipe(node: HTMLElement, initial: Options) {
    let options = initial;
    let pointer: Pointer | null = null;
    let target: HTMLElement | undefined;
    let distance = 0;
    let suppressClick = false;
    let animation: Animation | undefined;
    function available(eventTarget: EventTarget | null, x?: number) {
        if (!options.enabled || node.closest('[inert], [hidden]')) return false;
        if (!(eventTarget instanceof Element)) return true;
        if (
            eventTarget.closest(
                'input, textarea, select, a, button, [contenteditable], [data-ui-no-swipe], [role="slider"]',
            )
        )
            return false;
        if (
            eventTarget.closest('[data-ui-selectable]') &&
            x !== undefined &&
            node.getBoundingClientRect().right - x > 32
        )
            return false;
        for (
            let child: Element | null = eventTarget;
            child && child !== node;
            child = child.parentElement
        )
            if (
                child.scrollWidth > child.clientWidth + 1 &&
                /auto|scroll/.test(getComputedStyle(child).overflowX)
            )
                return false;
        return true;
    }
    function release() {
        const previous = pointer;
        pointer = null;
        if (previous && node.hasPointerCapture(previous.id))
            node.releasePointerCapture(previous.id);
    }
    function clear() {
        animation?.cancel();
        animation = undefined;
        if (target) {
            delete target.dataset.forwardPreview;
            target.style.removeProperty('--ui-forward-offset');
        }
        target = undefined;
        distance = 0;
    }
    function cancel() {
        release();
        clear();
    }
    function paint(value: number, width: number) {
        target ??= options.target();
        if (!target) return;
        distance = Math.min(width, Math.max(0, value));
        target.dataset.forwardPreview = 'true';
        target.style.setProperty('--ui-forward-offset', `${String(width - distance)}px`);
    }
    function settle(commit: boolean) {
        const layer = target;
        release();
        if (!layer) return;
        const from = node.clientWidth - distance;
        const finish = () => {
            if (commit && options.enabled) {
                options.onopen();
                void tick().then(clear);
            } else clear();
        };
        if (matchMedia('(prefers-reduced-motion: reduce)').matches) {
            finish();
            return;
        }
        animation = layer.animate(
            [
                { transform: `translate3d(${String(from)}px,0,0)` },
                { transform: `translate3d(${String(commit ? 0 : node.clientWidth)}px,0,0)` },
            ],
            { duration: 260, easing: 'cubic-bezier(0.2,0.8,0.2,1)', fill: 'both' },
        );
        animation.onfinish = finish;
    }
    function down(event: PointerEvent) {
        if (!event.isPrimary || event.button !== 0 || !available(event.target, event.clientX))
            return;
        cancel();
        suppressClick = false;
        const bounds = node.getBoundingClientRect();
        if (!bounds.width) return;
        pointer = {
            id: event.pointerId,
            x: event.clientX,
            y: event.clientY,
            width: node.clientWidth || bounds.width,
            scale: bounds.width / (node.clientWidth || bounds.width),
            dragging: false,
            lastX: event.clientX,
            time: event.timeStamp,
            velocity: 0,
        };
    }
    function move(event: PointerEvent) {
        const p = pointer;
        if (event.pointerId !== p?.id) return;
        const dx = (p.x - event.clientX) / p.scale;
        const dy = (event.clientY - p.y) / p.scale;
        if (!p.dragging) {
            if (Math.max(Math.abs(dx), Math.abs(dy)) < 8) return;
            if (dx < 0 || dx < Math.abs(dy) * 1.25) {
                release();
                return;
            }
            p.dragging = true;
            suppressClick = true;
            node.setPointerCapture(p.id);
            window.getSelection()?.removeAllRanges();
        }
        event.preventDefault();
        if (event.timeStamp > p.time)
            p.velocity = (p.lastX - event.clientX) / (event.timeStamp - p.time) / p.scale;
        p.lastX = event.clientX;
        p.time = event.timeStamp;
        paint(dx, p.width);
    }
    function up(event: PointerEvent) {
        const p = pointer;
        if (event.pointerId !== p?.id) return;
        if (!p.dragging) {
            release();
            return;
        }
        const velocity = event.timeStamp - p.time < 100 ? p.velocity : 0;
        settle(
            velocity >= -0.4 &&
                (distance >= Math.min(110, p.width * 0.25) || (distance > 24 && velocity > 0.55)),
        );
    }
    function interrupted(event: PointerEvent) {
        if (event.pointerId === pointer?.id) cancel();
    }
    function lostCapture(event: PointerEvent) {
        if (pointer?.dragging) interrupted(event);
    }
    function anotherTouch(event: PointerEvent) {
        if (pointer && event.pointerType === 'touch' && event.pointerId !== pointer.id) cancel();
    }
    function click(event: MouseEvent) {
        if (!suppressClick || event.detail === 0) return;
        suppressClick = false;
        event.preventDefault();
        event.stopImmediatePropagation();
    }
    const wheel = horizontalWheel(node, {
        accepts: (event) =>
            available(event.target) && (distance > 0 || event.deltaX > Math.abs(event.deltaY)),
        move: (value) => paint(-value, node.clientWidth),
        finish: () => settle(distance >= Math.min(110, node.clientWidth * 0.25)),
        cancel,
    });
    node.addEventListener('pointerdown', down);
    node.addEventListener('lostpointercapture', lostCapture);
    node.addEventListener('click', click, true);
    window.addEventListener('pointerdown', anotherTouch);
    window.addEventListener('pointermove', move, { passive: false });
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', interrupted);
    window.addEventListener('resize', cancel);
    window.addEventListener('blur', cancel);
    return {
        update(next: Options) {
            options = next;
            if (!next.enabled) cancel();
        },
        destroy() {
            wheel.destroy();
            cancel();
            node.removeEventListener('pointerdown', down);
            node.removeEventListener('lostpointercapture', lostCapture);
            node.removeEventListener('click', click, true);
            window.removeEventListener('pointerdown', anotherTouch);
            window.removeEventListener('pointermove', move);
            window.removeEventListener('pointerup', up);
            window.removeEventListener('pointercancel', interrupted);
            window.removeEventListener('resize', cancel);
            window.removeEventListener('blur', cancel);
        },
    };
}
