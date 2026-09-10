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

/** Resolve the previous mounted page, not the root screen behind the whole settings flow. */
export function navigationUnderlay(layer: HTMLElement): HTMLElement | null {
    if (layer.classList.contains('ui-editor-layer'))
        return layer.parentElement?.querySelector<HTMLElement>('.ui-track') ?? null;
    if (!layer.classList.contains('ui-overlay-layer')) return null;
    const container = layer.closest('.ui-management') ?? layer.parentElement;
    const layers = [...(container?.querySelectorAll<HTMLElement>('.ui-overlay-layer') ?? [])];
    const previous = layers
        .slice(0, layers.indexOf(layer))
        .reverse()
        .find((candidate) => !candidate.querySelector('[data-back-dismissed="true"]'));
    return (
        previous?.querySelector<HTMLElement>('.ui-overlay') ??
        container?.querySelector<HTMLElement>('.ui-management-content') ??
        null
    );
}

/** The child has already finished its interactive departure; don't add a second outro. */
export function pageSlide(layer: HTMLElement) {
    return ({ direction }: { direction: 'in' | 'out' } = { direction: 'in' }) => {
        if (direction === 'in' && layer.dataset.navigationCovered === 'true')
            return { duration: 0 };
        if (direction === 'out' && layer.querySelector('[data-back-dismissed="true"]'))
            return { duration: 0 };
        const width = layer.clientWidth;
        const timing = navigationTiming(width);
        const underlay = navigationUnderlay(layer);
        const originalTranslate = underlay?.style.translate ?? '';
        return {
            duration: window.matchMedia('(prefers-reduced-motion: reduce)').matches
                ? 0
                : timing.duration,
            easing: direction === 'out' ? (t: number) => 1 - timing.easing(1 - t) : timing.easing,
            css: (progress: number) =>
                `transform:translate3d(${String((1 - progress) * width)}px,0,0)`,
            tick: (progress: number) => {
                if (underlay)
                    underlay.style.translate =
                        progress === 0 || progress === 1
                            ? originalTranslate
                            : `${String(-0.28 * width * progress)}px 0`;
            },
        };
    };
}
