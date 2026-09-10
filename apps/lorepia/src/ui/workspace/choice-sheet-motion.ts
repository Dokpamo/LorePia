import { cubicOut } from 'svelte/easing';
import { fly } from 'svelte/transition';

export function dragOffset(panel: HTMLElement) {
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
    onexpanded?: (expanded: boolean) => void;
    enabled?: boolean;
}

export function setSheetExpanded(panel: HTMLElement, expanded: boolean) {
    panel.dataset.sheetExpanded = String(expanded);
    panel.style.setProperty('--ui-sheet-height', expanded ? '100%' : '70%');
    panel.style.setProperty('--ui-choice-drag', '0px');
}

/** Only the handle owns the two detents; the body retains native scrolling. */
export function choiceSheetDrag(handle: HTMLElement, initial: DragOptions) {
    let options = initial;
    let pointer: {
        id: number;
        panel: HTMLElement;
        y: number;
        origin: number;
        height: number;
        full: number;
        minimum: number;
        expanded: boolean;
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
        const { panel, expanded } = pointer;
        release();
        setSheetExpanded(panel, expanded);
    }
    function down(event: PointerEvent) {
        if (!event.isPrimary || event.button !== 0 || pointer || options.enabled === false) return;
        const panel = options.panel();
        if (panel.dataset.choiceClosing === 'true') return;
        const bounds = panel.getBoundingClientRect();
        const height = panel.offsetHeight || bounds.height;
        if (!height) return;
        const origin = dragOffset(panel);
        const scale = bounds.height / height;
        const full = (panel.parentElement?.getBoundingClientRect().height ?? bounds.height) / scale;
        panel.dataset.choiceDragging = 'true';
        panel.style.setProperty('--ui-sheet-height', `${String(height)}px`);
        panel.style.setProperty('--ui-choice-drag', `${String(origin)}px`);
        pointer = {
            id: event.pointerId,
            panel,
            y: event.clientY,
            origin,
            height,
            full,
            minimum: full * 0.7,
            expanded: panel.dataset.sheetExpanded === 'true',
            scale,
            lastY: event.clientY,
            lastTime: event.timeStamp,
            velocity: 0,
        };
        suppressClick = false;
        // Preserve native focus/click activation. The handle's touch-action
        // and user-select rules already reserve the drag without selection.
        handle.setPointerCapture(event.pointerId);
    }
    function distance(event: PointerEvent) {
        if (!pointer) return 0;
        return pointer.origin + (event.clientY - pointer.y) / pointer.scale;
    }
    function render(distance: number) {
        if (!pointer) return;
        const { panel, height, full, minimum, expanded } = pointer;
        const nextHeight = Math.max(expanded ? minimum : height, Math.min(full, height - distance));
        const offset = Math.max(0, Math.min(minimum, distance - (height - nextHeight)));
        panel.style.setProperty('--ui-sheet-height', `${String(nextHeight)}px`);
        panel.style.setProperty('--ui-choice-drag', `${String(offset)}px`);
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
        render(distance(event));
    }
    function up(event: PointerEvent) {
        if (event.pointerId !== pointer?.id) return;
        const offset = distance(event);
        const { panel, expanded, full, minimum } = pointer;
        const velocity = event.timeStamp - pointer.lastTime < 100 ? pointer.velocity : 0;
        const threshold = Math.min(96, (full - minimum) * 0.35);
        const upward = offset < -threshold || (offset < -24 && velocity < -0.55);
        const downward = offset > threshold || (offset > 24 && velocity > 0.55);
        const dismiss =
            !expanded &&
            velocity >= -0.4 &&
            (offset >= Math.min(160, minimum * 0.25) || (offset > 24 && velocity > 0.55));
        render(offset);
        release();
        if (dismiss) options.onclose();
        else {
            const next = expanded ? !downward : upward;
            setSheetExpanded(panel, next);
            options.onexpanded?.(next);
        }
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
        update(next: DragOptions) {
            options = next;
            if (next.enabled === false) cancel();
        },
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
