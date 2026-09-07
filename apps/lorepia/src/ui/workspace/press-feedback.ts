/** Measure on intent, so window resizing does not run a per-button layout loop. */
export function pressFeedback(root: HTMLElement) {
    let keyboardButton: HTMLButtonElement | null = null;
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
        if (button) measure(button);
    }
    function key(event: KeyboardEvent) {
        if (event.key !== ' ' && event.key !== 'Enter') return;
        const button = target(event);
        if (!button) return;
        measure(button);
        keyboardButton = button;
        button.dataset.uiPressed = 'true';
    }
    function clear() {
        if (keyboardButton) delete keyboardButton.dataset.uiPressed;
        keyboardButton = null;
    }
    root.addEventListener('pointerdown', pointer, true);
    root.addEventListener('keydown', key, true);
    root.addEventListener('focusout', clear);
    window.addEventListener('keyup', clear);
    window.addEventListener('blur', clear);
    return {
        destroy() {
            clear();
            root.removeEventListener('pointerdown', pointer, true);
            root.removeEventListener('keydown', key, true);
            root.removeEventListener('focusout', clear);
            window.removeEventListener('keyup', clear);
            window.removeEventListener('blur', clear);
        },
    };
}
