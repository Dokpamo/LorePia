/** Keep keyboard navigation in the current modal, including single-line search. */
export function trapFocus(event: KeyboardEvent & { currentTarget: EventTarget & HTMLElement }) {
    if (event.key !== 'Tab') return;
    const controls = [
        ...event.currentTarget.querySelectorAll<HTMLElement>(
            'button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), a[href], [tabindex="0"]',
        ),
    ].filter(
        (node) => node.tabIndex >= 0 && !node.closest('[inert], [hidden], [aria-hidden="true"]'),
    );
    const first = controls[0];
    const last = controls.at(-1);
    if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus({ preventScroll: true });
    } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus({ preventScroll: true });
    }
}
