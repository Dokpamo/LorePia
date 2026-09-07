/** A critically damped settle: no overshoot that could expose the window edge. */
export function navigationTiming(distance: number, velocity = 0) {
    const duration = Math.round(
        Math.max(200, Math.min(420, 300 + Math.abs(distance) * 0.2 - Math.abs(velocity) * 70)),
    );
    const initial = Math.min(
        7,
        Math.max(0, (velocity * duration) / Math.max(1, Math.abs(distance))),
    );
    const end = 1 - (9 - initial) * Math.exp(-8);
    const easing = (time: number) => (1 - (1 + (8 - initial) * time) * Math.exp(-8 * time)) / end;
    const points = Array.from({ length: 25 }, (_, index) => easing(index / 24).toFixed(5));
    return { duration, easing, cssEasing: `linear(${points.join(',')})` };
}

export function setNavigationTiming(node: HTMLElement, distance: number, velocity = 0) {
    const timing = navigationTiming(distance, velocity);
    node.style.setProperty('--ui-navigation-duration', `${String(timing.duration)}ms`);
    node.style.setProperty('--ui-navigation-easing', timing.cssEasing);
}

/** The child has already finished its interactive departure; don't add a second outro. */
export function pageSlide(layer: HTMLElement) {
    return ({ direction }: { direction: 'in' | 'out' } = { direction: 'in' }) => {
        if (direction === 'out' && layer.querySelector('[data-back-dismissed="true"]'))
            return { duration: 0 };
        const width = layer.clientWidth;
        const timing = navigationTiming(width);
        return {
            duration: window.matchMedia('(prefers-reduced-motion: reduce)').matches
                ? 0
                : timing.duration,
            easing: direction === 'out' ? (t: number) => 1 - timing.easing(1 - t) : timing.easing,
            css: (progress: number) =>
                `transform:translate3d(${String((1 - progress) * width)}px,0,0)`,
        };
    };
}
