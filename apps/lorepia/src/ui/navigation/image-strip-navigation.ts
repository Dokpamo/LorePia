import { imageStripDrag } from './image-strip-drag';

/** The center is the selection slot. Scrolling previews it; settling aligns it exactly. */
export function imageStripNavigation(
    node: HTMLElement,
    initialIndex: number,
    onselect: (index: number, browsing: boolean) => void,
) {
    let current = initialIndex;
    let browsing = false;
    let userScrolling = false;
    let touching = false;
    let frame = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let buttons: HTMLButtonElement[] = [];
    let centers: number[] = [];
    let step = 1;

    function stop() {
        cancelAnimationFrame(frame);
        frame = 0;
        clearTimeout(timer);
    }
    function measure() {
        buttons = [...node.querySelectorAll('button')];
        centers = buttons.map((button) => button.offsetLeft + button.offsetWidth / 2);
        step = Math.max(
            1,
            centers[1] === undefined
                ? (buttons[0]?.offsetWidth ?? 1)
                : centers[1] - (centers[0] ?? 0),
        );
    }
    function position(index: number) {
        return Math.max(
            0,
            Math.min(
                node.scrollWidth - node.clientWidth,
                (centers[index] ?? 0) - node.clientWidth / 2,
            ),
        );
    }
    function paint() {
        const middle = node.scrollLeft + node.clientWidth / 2;
        let nearest = current;
        let distance = Infinity;
        centers.forEach((center, index) => {
            const delta = Math.abs(center - middle);
            if (delta < distance) {
                distance = delta;
                nearest = index;
            }
            buttons[index]?.style.setProperty(
                '--image-focus',
                Math.max(0, 1 - delta / step).toFixed(3),
            );
        });
        return nearest;
    }
    function report(index: number, active: boolean) {
        if (current === index && browsing === active) return;
        current = index;
        browsing = active;
        if (node.contains(document.activeElement)) buttons[index]?.focus({ preventScroll: true });
        onselect(index, active);
    }
    function center(index: number, animate: boolean) {
        stop();
        drag.cancel();
        userScrolling = false;
        const from = node.scrollLeft;
        const to = position(index);
        const finish = () => {
            node.scrollLeft = to;
            paint();
            frame = 0;
            report(index, false);
        };
        if (
            !node.clientWidth ||
            !animate ||
            Math.abs(to - from) < 0.5 ||
            matchMedia('(prefers-reduced-motion: reduce)').matches
        ) {
            finish();
            return;
        }
        const start = performance.now();
        function step(now: number) {
            const progress = Math.min(1, (now - start) / 180);
            node.scrollLeft = from + (to - from) * (1 - (1 - progress) ** 3);
            paint();
            if (progress < 1) frame = requestAnimationFrame(step);
            else finish();
        }
        frame = requestAnimationFrame(step);
    }
    function settle() {
        if (!userScrolling || touching || drag.active || !node.clientWidth) return;
        const nearest = paint();
        report(nearest, true);
        center(nearest, true);
    }
    function schedule() {
        clearTimeout(timer);
        timer = setTimeout(settle, 120);
    }
    function begin() {
        stop();
        userScrolling = true;
        schedule();
    }
    function scroll() {
        const nearest = paint();
        if (!userScrolling || !node.clientWidth) return;
        report(nearest, true);
        schedule();
    }
    function touchStart() {
        touching = true;
    }
    function touchEnd() {
        touching = false;
        schedule();
    }
    const drag = imageStripDrag(node, begin, scroll, settle);
    function resize() {
        measure();
        center(current, false);
    }
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(resize);
    observer?.observe(node);
    node.addEventListener('scroll', scroll, { passive: true });
    node.addEventListener('scrollend', settle);
    node.addEventListener('touchstart', touchStart, { passive: true });
    node.addEventListener('touchend', touchEnd, { passive: true });
    node.addEventListener('touchcancel', touchEnd, { passive: true });
    window.addEventListener('resize', resize);
    window.addEventListener('blur', resize);
    resize();
    return {
        sync(index: number, imagesChanged = false) {
            if (imagesChanged) measure();
            // A preview reported by scrolling is echoed through Svelte props, not a new command.
            if (index === current && !imagesChanged) return;
            current = index;
            center(index, true);
        },
        choose(index: number) {
            report(index, false);
            center(index, true);
        },
        destroy() {
            stop();
            drag.destroy();
            observer?.disconnect();
            node.removeEventListener('scroll', scroll);
            node.removeEventListener('scrollend', settle);
            node.removeEventListener('touchstart', touchStart);
            node.removeEventListener('touchend', touchEnd);
            node.removeEventListener('touchcancel', touchEnd);
            window.removeEventListener('resize', resize);
            window.removeEventListener('blur', resize);
        },
    };
}
