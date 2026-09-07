import type { Page } from './view-types';

export interface ViewportSize {
    width: number;
    height: number;
}

export type LayoutMode = 'mobile' | 'split' | 'wide';

export function layoutMode(width: number) {
    return width >= 1120 ? 'wide' : width >= 760 ? 'split' : 'mobile';
}

export function responsiveLayout(
    size: ViewportSize,
    page: Page,
    subpage: boolean,
    mode: LayoutMode = layoutMode(size.width),
) {
    // Reflow into the available width; keep text and hit targets at their CSS size.
    const width = Math.max(1, size.width);
    const scale = 1;
    const selected = page === 2 && !subpage ? 1 : page;
    const left = mode === 'mobile' ? width : Math.min(360, Math.max(300, width * 0.3));
    const right = !subpage
        ? 0
        : mode === 'mobile'
          ? width
          : mode === 'split'
            ? left
            : Math.min(320, Math.max(280, width * 0.24));
    const rightSide = mode === 'split' && selected === 2;
    const chat =
        mode === 'mobile'
            ? width
            : mode === 'wide'
              ? width - left - right
              : width - (rightSide ? right : left);
    const offset = mode === 'mobile' ? -selected * width : rightSide ? -left : 0;
    const visible: Page[] =
        mode === 'mobile'
            ? [selected]
            : rightSide
              ? [1, 2]
              : mode === 'wide' && subpage
                ? [0, 1, 2]
                : [0, 1];
    return {
        mode,
        width,
        height: Math.max(1, size.height) / scale,
        scale,
        left,
        chat,
        right,
        offset,
        visible,
    };
}
