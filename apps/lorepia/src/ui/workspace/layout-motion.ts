import { responsiveLayout, type LayoutMode, type ViewportSize } from './responsive-layout';
import type { Page } from './view-types';

const modes: LayoutMode[] = ['mobile', 'split', 'wide'];
const pages: Page[] = [0, 1, 2];
const frames = modes.flatMap((mode) => pages.map((page) => ({ mode, page })));

export function layoutWeights(mode: LayoutMode, page: Page) {
    return frames.map((frame) => Number(frame.mode === mode && frame.page === page));
}

// Animate composition, not stale pixel widths. Every source/target geometry is
// recalculated against the live viewport, so resizing never restarts the tween.
export function blendLayout(size: ViewportSize, subpage: boolean, weights: number[]) {
    const result = { left: 0, chat: 0, right: 0, offset: 0 };
    frames.forEach((frame, index) => {
        const weight = weights[index] ?? 0;
        if (!weight) return;
        // A zoom can jump several breakpoints in one frame. Keep the outgoing
        // composition valid even when its columns no longer fit their minima.
        const minimum = frame.mode === 'wide' ? 1120 : frame.mode === 'split' ? 760 : 1;
        const width = Math.max(minimum, size.width);
        const layout = responsiveLayout({ ...size, width }, frame.page, subpage, frame.mode);
        const contribution = weight * (Math.max(1, size.width) / width);
        result.left += layout.left * contribution;
        result.chat += layout.chat * contribution;
        result.right += layout.right * contribution;
        result.offset += layout.offset * contribution;
    });
    return result;
}
