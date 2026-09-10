/** A scroll-driven header leaves its space in the content, so hiding never leaves a blank bar. */
export function scrollHeaderMotion(header: HTMLElement, body: HTMLElement) {
    const frame = body.parentElement;
    const floating = getComputedStyle(header).position === 'absolute';
    const previousPosition = frame?.style.position ?? '';
    const previousPadding = body.style.paddingTop;
    let height = 0;
    let offset = 0;
    let previous = body.scrollTop;
    let inputUntil = -Infinity;
    let direction = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let animationTimer: ReturnType<typeof setTimeout> | undefined;
    if (frame) frame.dataset.scrollHeaderFrame = '';
    if (frame && getComputedStyle(frame).position === 'static') frame.style.position = 'relative';
    header.dataset.scrollHeader = '';
    if (!floating) header.dataset.scrollHeaderFloating = '';

    function title() {
        const source = body.querySelector<HTMLElement>('[data-scroll-title]');
        if (!source) return;
        const scale = body.getBoundingClientRect().height / body.offsetHeight || 1;
        header.dataset.titleCollapsed = String(
            source.getBoundingClientRect().bottom <=
                body.getBoundingClientRect().top + height * scale,
        );
    }
    function paint(next: number, animate = false) {
        offset = Math.max(0, Math.min(height, next));
        clearTimeout(animationTimer);
        header.dataset.scrollHeaderSettling = String(animate);
        header.style.setProperty('--ui-scroll-header-offset', `${String(offset)}px`);
        if (animate)
            animationTimer = setTimeout(() => {
                header.dataset.scrollHeaderSettling = 'false';
            }, 180);
    }
    function show() {
        clearTimeout(timer);
        inputUntil = -Infinity;
        paint(0, true);
    }
    function measure() {
        const top = body.scrollTop;
        height = header.offsetHeight;
        if (!floating) {
            delete body.dataset.scrollHeaderBody;
            body.style.paddingTop = previousPadding;
            const padding = getComputedStyle(body).paddingTop;
            body.style.setProperty('--ui-scroll-body-padding', padding);
            body.style.setProperty('--ui-scroll-header-height', `${String(height)}px`);
            body.dataset.scrollHeaderBody = '';
            body.style.paddingTop =
                'calc(var(--ui-scroll-body-padding, 0px) + var(--ui-scroll-header-height, 0px))';
        }
        body.scrollTop = top;
        previous = top;
        paint(Math.min(offset, height));
        title();
    }
    function settle() {
        const hide = direction > 0 && offset > height * 0.35 && body.scrollTop > height;
        paint(hide ? height : 0, true);
    }
    function scroll() {
        const top = Math.max(0, Math.min(body.scrollHeight - body.clientHeight, body.scrollTop));
        const delta = top - previous;
        previous = top;
        title();
        if (body.closest('[inert], [aria-hidden="true"]')) return;
        if (top <= 0) {
            show();
            return;
        }
        // Stream updates, focus restoration and resize are not scroll gestures.
        if (performance.now() > inputUntil || Math.abs(delta) < 0.5) return;
        direction = Math.sign(delta);
        paint(offset + delta);
        clearTimeout(timer);
        timer = setTimeout(settle, 120);
    }
    header.addEventListener('focusin', show);
    header.addEventListener('input', show);
    body.addEventListener('scroll', scroll, { passive: true });
    body.addEventListener('scrollend', settle);
    const observer =
        typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(measure);
    observer?.observe(header);
    window.addEventListener('resize', measure);
    measure();
    return {
        input() {
            inputUntil = performance.now() + 1600;
        },
        show,
        destroy() {
            clearTimeout(timer);
            clearTimeout(animationTimer);
            observer?.disconnect();
            window.removeEventListener('resize', measure);
            body.removeEventListener('scroll', scroll);
            body.removeEventListener('scrollend', settle);
            header.removeEventListener('focusin', show);
            header.removeEventListener('input', show);
            delete header.dataset.scrollHeader;
            delete header.dataset.scrollHeaderFloating;
            delete header.dataset.scrollHeaderSettling;
            delete header.dataset.titleCollapsed;
            delete body.dataset.scrollHeaderBody;
            header.style.removeProperty('--ui-scroll-header-offset');
            body.style.removeProperty('--ui-scroll-body-padding');
            body.style.removeProperty('--ui-scroll-header-height');
            body.style.paddingTop = previousPadding;
            if (frame) {
                delete frame.dataset.scrollHeaderFrame;
                frame.style.position = previousPosition;
            }
        },
    };
}
