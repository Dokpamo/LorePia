/** Measure on intent, so window resizing does not run a per-button layout loop. */
export function pressFeedback(root: HTMLElement) {
    let keyboardButton: HTMLButtonElement | null = null;
    let imagePointer: { button: HTMLButtonElement; id: number; x: number; y: number } | null = null;
    function target(event: Event) {
        const button =
            event.target instanceof Element
                ? event.target.closest<HTMLButtonElement>('button.ui-pressable')
                : null;
        return button && root.contains(button) && !button.disabled ? button : null;
    }
    function measure(button: HTMLButtonElement) {
        const basis = Math.max(button.offsetHeight, button.offsetWidth / 4, 24);
        button.style.setProperty('--ui-press-scale', String((basis - 2) / basis));
    }
    function pointer(event: PointerEvent) {
        clear();
        if (!event.isPrimary || event.button !== 0) return;
        const button = target(event);
        if (!button) return;
        measure(button);
        if (button.dataset.pressFeedback === 'scale') {
            imagePointer = { button, id: event.pointerId, x: event.clientX, y: event.clientY };
            button.dataset.uiPressed = 'true';
        }
    }
    function cancelImagePress() {
        if (!imagePointer) return;
        delete imagePointer.button.dataset.uiPressed;
        // Native :active can remain set during a scroll or a captured image pan.
        imagePointer.button.dataset.uiPressCancelled = '';
    }
    function move(event: PointerEvent) {
        if (imagePointer?.id !== event.pointerId) return;
        if (Math.hypot(event.clientX - imagePointer.x, event.clientY - imagePointer.y) >= 6)
            cancelImagePress();
    }
    function release(event: PointerEvent) {
        if (imagePointer?.id === event.pointerId) clearImagePress();
    }
    function loseCapture(event: PointerEvent) {
        if (imagePointer?.id === event.pointerId) cancelImagePress();
    }
    function clearImagePress() {
        if (imagePointer) {
            delete imagePointer.button.dataset.uiPressed;
            delete imagePointer.button.dataset.uiPressCancelled;
        }
        imagePointer = null;
    }
    function key(event: KeyboardEvent) {
        if (event.key !== ' ' && event.key !== 'Enter') return;
        const button = target(event);
        if (!button) return;
        clear();
        measure(button);
        keyboardButton = button;
        button.dataset.uiPressed = 'true';
    }
    function clearKeyboard() {
        if (keyboardButton) delete keyboardButton.dataset.uiPressed;
        keyboardButton = null;
    }
    function clear() {
        clearKeyboard();
        clearImagePress();
    }
    root.addEventListener('pointerdown', pointer, true);
    root.addEventListener('keydown', key, true);
    root.addEventListener('focusout', clearKeyboard);
    window.addEventListener('keyup', clearKeyboard);
    window.addEventListener('pointermove', move, true);
    window.addEventListener('pointerup', release, true);
    window.addEventListener('pointercancel', release, true);
    window.addEventListener('lostpointercapture', loseCapture, true);
    window.addEventListener('blur', clear);
    return {
        destroy() {
            clear();
            root.removeEventListener('pointerdown', pointer, true);
            root.removeEventListener('keydown', key, true);
            root.removeEventListener('focusout', clearKeyboard);
            window.removeEventListener('keyup', clearKeyboard);
            window.removeEventListener('pointermove', move, true);
            window.removeEventListener('pointerup', release, true);
            window.removeEventListener('pointercancel', release, true);
            window.removeEventListener('lostpointercapture', loseCapture, true);
            window.removeEventListener('blur', clear);
        },
    };
}
