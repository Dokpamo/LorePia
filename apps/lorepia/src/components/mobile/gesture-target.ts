/** Keep native editing, sliders, and selection controls in charge of their drag. */
export function isEditingGestureTarget(target: EventTarget | null): boolean {
    return (
        target instanceof Element &&
        target.closest(
            'input, textarea, select, [role="slider"], [role="switch"], [role="radio"], [role="combobox"], [contenteditable="true"]',
        ) !== null
    );
}
