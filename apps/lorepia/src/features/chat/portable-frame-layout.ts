import type { PortableRendererMessage, PortableRegion } from './portable-renderer-protocol';

/** Only the visible card regions paint/receive input; transparent room space belongs to chat. */
export function portableRegionPath(
    regions: PortableRegion[],
    width: number,
    height: number,
): string {
    const paths = regions
        .map((region) => {
            const x = Math.min(width, region.x);
            const y = Math.min(height, region.y);
            const right = Math.min(width, region.x + region.width);
            const bottom = Math.min(height, region.y + region.height);
            return right > x && bottom > y
                ? `M${String(x)},${String(y)}H${String(right)}V${String(bottom)}H${String(x)}Z`
                : '';
        })
        .join('');
    return `path("${paths || 'M0,0Z'}")`;
}

export function applyPortableFrameLayout(
    frame: HTMLIFrameElement,
    message: PortableRendererMessage,
    room: boolean,
    floating: boolean,
): void {
    if (message.type === 'portable_resize' && !room)
        frame.style.height = `${String(message.height)}px`;
    if (message.type === 'portable_regions' && room && floating)
        frame.style.clipPath = portableRegionPath(
            message.regions,
            frame.clientWidth,
            frame.clientHeight,
        );
}
