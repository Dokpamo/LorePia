import { cubicOut } from 'svelte/easing';
import { fly } from 'svelte/transition';

function dragOffset(panel: HTMLElement) {
    const translate = getComputedStyle(panel).translate.split(/\s+/);
    return (
        Number.parseFloat(translate[1] ?? '') ||
        Number.parseFloat(panel.style.getPropertyValue('--ui-choice-drag')) ||
        0
    );
}

/** Continue the exit from the released position to below the containing screen. */
export function choiceSheetTransition(panel: HTMLElement, reduced: boolean) {
    const offset = dragOffset(panel);
    if (offset) {
        panel.style.transition = 'none';
        panel.style.setProperty('--ui-choice-drag', `${String(offset)}px`);
    }
    const bounds = panel.getBoundingClientRect();
    const scale = bounds.height / (panel.offsetHeight || bounds.height || 1) || 1;
    const bottom = panel.parentElement?.getBoundingClientRect().bottom ?? bounds.bottom;
    return fly(panel, {
        y: Math.max(0, (bottom - bounds.top) / scale),
        opacity: 1,
        duration: reduced ? 0 : 300,
        easing: cubicOut,
    });
}

interface DragOptions {
    panel: () => HTMLElement;
    onclose: () => void;
}

/** Only the handle owns the gesture; the choice list retains normal scrolling. */
export function choiceSheetDrag(handle: HTMLElement, options: DragOptions) {
    let pointer: {
        id: number;
        panel: HTMLElement;
        y: number;
        origin: number;
        height: number;
        scale: number;
        lastY: number;
        lastTime: number;
        velocity: number;
    } | null = null;
    let suppressClick = false;

    function release() {
        const active = pointer;
        pointer = null;
        if (!active) return;
        delete active.panel.dataset.choiceDragging;
        if (handle.hasPointerCapture(active.id)) handle.releasePointerCapture(active.id);
    }
    function cancel() {
        if (!pointer) return;
        const panel = pointer.panel;
        release();
        panel.style.setProperty('--ui-choice-drag', '0px');
    }
    function down(event: PointerEvent) {
        if (!event.isPrimary || event.button !== 0 || pointer) return;
        const panel = options.panel();
        if (panel.dataset.choiceClosing === 'true') return;
        const bounds = panel.getBoundingClientRect();
        const height = panel.offsetHeight || bounds.height;
        if (!height) return;
        const origin = dragOffset(panel);
        panel.dataset.choiceDragging = 'true';
        panel.style.setProperty('--ui-choice-drag', `${String(origin)}px`);
        pointer = {
            id: event.pointerId,
            panel,
            y: event.clientY,
            origin,
            height,
            scale: bounds.height / height,
            lastY: event.clientY,
            lastTime: event.timeStamp,
            velocity: 0,
        };
        suppressClick = false;
        event.preventDefault();
        handle.setPointerCapture(event.pointerId);
    }
    function distance(event: PointerEvent) {
        if (!pointer) return 0;
        return Math.max(
            0,
            Math.min(pointer.height, pointer.origin + (event.clientY - pointer.y) / pointer.scale),
        );
    }
    function move(event: PointerEvent) {
        if (event.pointerId !== pointer?.id) return;
        event.preventDefault();
        const elapsed = event.timeStamp - pointer.lastTime;
        if (elapsed > 0)
            pointer.velocity = (event.clientY - pointer.lastY) / elapsed / pointer.scale;
        pointer.lastY = event.clientY;
        pointer.lastTime = event.timeStamp;
        if (Math.abs(event.clientY - pointer.y) > 4) suppressClick = true;
        pointer.panel.style.setProperty('--ui-choice-drag', `${String(distance(event))}px`);
    }
    function up(event: PointerEvent) {
        if (event.pointerId !== pointer?.id) return;
        const offset = distance(event);
        const commit =
            offset >= Math.min(160, pointer.height * 0.25) ||
            (offset > 24 && pointer.velocity > 0.55 && event.timeStamp - pointer.lastTime < 100);
        const panel = pointer.panel;
        release();
        panel.style.setProperty('--ui-choice-drag', `${String(commit ? offset : 0)}px`);
        if (commit) options.onclose();
    }
    function interrupted(event: PointerEvent) {
        if (event.pointerId === pointer?.id) cancel();
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
    handle.addEventListener('pointerdown', down);
    handle.addEventListener('lostpointercapture', interrupted);
    handle.addEventListener('click', click, true);
    window.addEventListener('pointerdown', anotherTouch);
    window.addEventListener('pointermove', move, { passive: false });
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', interrupted);
    window.addEventListener('resize', cancel);
    return {
        destroy() {
            cancel();
            handle.removeEventListener('pointerdown', down);
            handle.removeEventListener('lostpointercapture', interrupted);
            handle.removeEventListener('click', click, true);
            window.removeEventListener('pointerdown', anotherTouch);
            window.removeEventListener('pointermove', move);
            window.removeEventListener('pointerup', up);
            window.removeEventListener('pointercancel', interrupted);
            window.removeEventListener('resize', cancel);
        },
    };
}
