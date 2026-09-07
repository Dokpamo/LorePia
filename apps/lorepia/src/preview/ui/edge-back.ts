import { horizontalWheel } from './horizontal-wheel';
import { navigationTiming } from './navigation-motion';

export function requestBack(node: HTMLElement) {
    node.dispatchEvent(new Event('ui-back'));
}

interface BackOptions {
    onback: () => void;
    enabled?: boolean;
}

/** Swipe free surfaces; an editor's text region keeps its native selection. */
export function edgeBack(node: HTMLElement, initial: BackOptions) {
    let options = initial;
    let pointer: {
        id: number;
        x: number;
        y: number;
        scale: number;
        width: number;
        time: number;
        origin: number;
        velocity: number;
        lastX: number;
        lastTime: number;
    } | null = null;
    let dragging = false;
    let suppressClick = false;
    let animation: Animation | undefined;
    let underlayAnimation: Animation | undefined;
    let settlingCommit = false;
    const layer = node.parentElement;
    const underlay = layer?.classList.contains('ui-editor-layer')
        ? layer.parentElement?.querySelector<HTMLElement>('.ui-track')
        : layer?.classList.contains('ui-overlay-layer')
          ? layer.parentElement?.querySelector<HTMLElement>('.ui-management-content')
          : null;
    const originalTranslate = underlay?.style.translate ?? '';

    function visibleOffset() {
        const transform = getComputedStyle(node).transform;
        const match = /matrix(3d)?\(([^)]+)\)/.exec(transform);
        const values = match?.[2]?.split(',').map(Number);
        return (
            values?.[match?.[1] ? 12 : 4] ??
            (Number.parseFloat(node.style.getPropertyValue('--ui-back-offset')) || 0)
        );
    }

    function restoreUnderlay() {
        underlayAnimation?.cancel();
        underlayAnimation = undefined;
        if (underlay) underlay.style.translate = originalTranslate;
    }

    function render(distance: number) {
        if (!pointer) return;
        const width = pointer.width / pointer.scale;
        node.style.setProperty('--ui-back-offset', `${String(distance)}px`);
        if (underlay)
            underlay.style.translate = `${String(-0.28 * Math.max(0, width - distance))}px 0`;
    }

    function clear() {
        const id = pointer?.id;
        pointer = null;
        dragging = false;
        delete node.dataset.backDragging;
        node.style.removeProperty('--ui-back-offset');
        if (id !== undefined && id >= 0 && node.hasPointerCapture(id))
            node.releasePointerCapture(id);
    }

    function down(event: PointerEvent) {
        const origin = visibleOffset();
        clear();
        suppressClick = false;
        if (options.enabled === false || !event.isPrimary || event.button !== 0) return;
        const bounds = node.getBoundingClientRect();
        if (!bounds.width || event.clientX < bounds.left) return;
        const editing =
            event.target instanceof Element &&
            event.target.closest(
                'textarea, input, select, [contenteditable], [data-ui-selectable]',
            );
        if (editing && event.clientX - bounds.left > 32) return;
        animation?.cancel();
        animation = undefined;
        underlayAnimation?.cancel();
        delete node.dataset.backSettling;
        delete node.dataset.backDismissed;
        pointer = {
            id: event.pointerId,
            x: event.clientX,
            y: event.clientY,
            scale: bounds.width / (node.clientWidth || bounds.width),
            width: bounds.width,
            time: event.timeStamp,
            origin,
            velocity: 0,
            lastX: event.clientX,
            lastTime: event.timeStamp,
        };
        if (!(event.target instanceof Element && event.target.closest('button, a'))) {
            // WebKit starts native text tracking on pointerdown and can consume
            // all following moves. Reserve the accepted edge/free surface now;
            // the editable interior returned above keeps native selection.
            event.preventDefault();
            node.setPointerCapture(event.pointerId);
        }
        if (origin) render(origin);
    }

    function move(event: PointerEvent) {
        if (event.pointerId !== pointer?.id) return;
        const dx = event.clientX - pointer.x;
        const dy = event.clientY - pointer.y;
        if (!dragging) {
            if (Math.max(Math.abs(dx), Math.abs(dy)) < 8) return;
            if ((dx < 0 && pointer.origin === 0) || Math.abs(dx) < Math.abs(dy) * 1.25) {
                if (pointer.origin) settle(false);
                else {
                    clear();
                    restoreUnderlay();
                }
                return;
            }
            dragging = true;
            suppressClick = true;
            node.dataset.backDragging = 'true';
            node.setPointerCapture(event.pointerId);
        }
        event.preventDefault();
        const elapsed = event.timeStamp - pointer.lastTime;
        if (elapsed > 0) pointer.velocity = (event.clientX - pointer.lastX) / elapsed;
        pointer.lastX = event.clientX;
        pointer.lastTime = event.timeStamp;
        render(
            Math.max(
                0,
                Math.min(pointer.origin + dx / pointer.scale, pointer.width / pointer.scale),
            ),
        );
    }

    function settle(commit: boolean) {
        if (!pointer) return;
        const from = visibleOffset();
        const width = pointer.width / pointer.scale;
        const to = commit ? width : 0;
        const timing = navigationTiming(
            to - from,
            Math.max(0, (pointer.velocity / pointer.scale) * (commit ? 1 : -1)),
        );
        const fromUnderlay = -0.28 * (width - from);
        const toUnderlay = commit ? 0 : -0.28 * width;
        clear();
        animation?.cancel();
        underlayAnimation?.cancel();
        settlingCommit = commit;
        const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
        const animationOptions = {
            duration: reduced ? 0 : timing.duration,
            easing: timing.cssEasing,
            fill: 'forwards' as const,
        };
        node.dataset.backSettling = 'true';
        animation = node.animate(
            [
                { transform: `translate3d(${String(from)}px,0,0)` },
                { transform: `translate3d(${String(to)}px,0,0)` },
            ],
            animationOptions,
        );
        underlayAnimation = underlay?.animate(
            [
                { translate: `${String(fromUnderlay)}px 0` },
                { translate: `${String(toUnderlay)}px 0` },
            ],
            animationOptions,
        );
        const settling = animation;
        void animation.finished
            .then(() => {
                if (animation !== settling) return;
                delete node.dataset.backSettling;
                restoreUnderlay();
                if (commit) {
                    node.dataset.backDismissed = 'true';
                    // Hold the offscreen frame through any component outro.
                    // Clearing it here would flash the editor back into view.
                    options.onback();
                    return;
                }
                settling.cancel();
                animation = undefined;
            })
            .catch(() => undefined);
    }

    function up(event: PointerEvent) {
        if (event.pointerId !== pointer?.id) return;
        if (!dragging) {
            if (pointer.origin) settle(settlingCommit);
            else clear();
            return;
        }
        const distance = pointer.origin * pointer.scale + event.clientX - pointer.x;
        const elapsed = event.timeStamp - pointer.time;
        settle(
            pointer.velocity >= -0.4 &&
                (distance >= Math.min(110, pointer.width * 0.28) ||
                    (distance > 32 && elapsed > 0 && distance / elapsed > 0.55)),
        );
    }
    function cancel(event?: PointerEvent) {
        if (event && event.pointerId !== pointer?.id) return;
        animation?.cancel();
        animation = undefined;
        delete node.dataset.backSettling;
        restoreUnderlay();
        clear();
    }
    function click(event: MouseEvent) {
        if (!suppressClick || event.detail === 0) return;
        suppressClick = false;
        event.preventDefault();
        event.stopImmediatePropagation();
    }
    const interrupt = () => cancel();
    function close() {
        if (options.enabled === false) return;
        const bounds = node.getBoundingClientRect();
        if (!bounds.width) {
            options.onback();
            return;
        }
        pointer = {
            id: -1,
            x: 0,
            y: 0,
            width: bounds.width,
            scale: bounds.width / (node.clientWidth || bounds.width),
            time: 0,
            origin: visibleOffset(),
            velocity: 0,
            lastX: 0,
            lastTime: 0,
        };
        settle(true);
    }
    function keydown(event: KeyboardEvent) {
        if (event.key !== 'Escape') return;
        event.preventDefault();
        event.stopImmediatePropagation();
        close();
    }
    const wheel = horizontalWheel(node, {
        accepts: () => options.enabled !== false,
        move(distance) {
            const bounds = node.getBoundingClientRect();
            if (!pointer) {
                const origin = visibleOffset();
                animation?.cancel();
                animation = undefined;
                underlayAnimation?.cancel();
                delete node.dataset.backSettling;
                pointer = {
                    id: -1,
                    x: 0,
                    y: 0,
                    scale: bounds.width / (node.clientWidth || bounds.width),
                    width: bounds.width,
                    time: 0,
                    origin,
                    velocity: 0,
                    lastX: 0,
                    lastTime: 0,
                };
            }
            node.dataset.backDragging = 'true';
            render(
                Math.max(
                    0,
                    Math.min(
                        pointer.origin + distance / pointer.scale,
                        pointer.width / pointer.scale,
                    ),
                ),
            );
        },
        finish(distance) {
            settle(distance >= 64);
        },
        cancel: interrupt,
    });
    node.addEventListener('pointerdown', down);
    node.addEventListener('ui-back', close);
    node.addEventListener('keydown', keydown);
    node.addEventListener('lostpointercapture', cancel);
    node.addEventListener('click', click, true);
    window.addEventListener('pointermove', move, { passive: false });
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', cancel);
    window.addEventListener('blur', interrupt);
    window.addEventListener('resize', interrupt);
    return {
        update(next: BackOptions) {
            options = next;
            if (next.enabled === false) {
                cancel();
                wheel.cancel();
            }
        },
        destroy() {
            cancel();
            wheel.destroy();
            node.removeEventListener('pointerdown', down);
            node.removeEventListener('ui-back', close);
            node.removeEventListener('keydown', keydown);
            node.removeEventListener('lostpointercapture', cancel);
            node.removeEventListener('click', click, true);
            window.removeEventListener('pointermove', move);
            window.removeEventListener('pointerup', up);
            window.removeEventListener('pointercancel', cancel);
            window.removeEventListener('blur', interrupt);
            window.removeEventListener('resize', interrupt);
        },
    };
}
