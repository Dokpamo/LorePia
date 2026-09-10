/** Focus must not scroll the sheet's animated ancestors into the viewport. */
export function focusChoiceOption(option?: HTMLElement | null) {
    if (!option) return;
    option.focus({ preventScroll: true });
    const list = option.closest<HTMLElement>('[role="radiogroup"]');
    if (!list?.clientHeight) return;
    const padding = Number.parseFloat(getComputedStyle(list).scrollPaddingTop) || 0;
    // The positioned list is the offset parent, so these coordinates exclude
    // the sheet's entrance transform and the application's responsive scale.
    const top = option.offsetTop;
    const bottom = top + option.offsetHeight;
    if (top < list.scrollTop + padding) {
        list.scrollTop = Math.max(0, top - padding);
    } else if (bottom > list.scrollTop + list.clientHeight - padding) {
        list.scrollTop = Math.max(0, Math.min(top - padding, bottom - list.clientHeight + padding));
    }
}
