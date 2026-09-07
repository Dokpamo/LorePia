import { describe, expect, it } from 'vitest';
import { blendLayout, layoutWeights } from './layout-motion';
import { responsiveLayout, type LayoutMode } from './responsive-layout';
import type { Page } from './sample-data';

const states = (['mobile', 'split', 'wide'] as LayoutMode[]).flatMap((mode) =>
    ([0, 1, 2] as Page[]).map((page) => layoutWeights(mode, page)),
);

describe('responsive composition motion', () => {
    it.each([320, 393, 759, 760, 1119, 1120, 1440])(
        'keeps columns positive and the viewport covered throughout transitions at %ipx',
        (width) => {
            for (const subpage of [true, false])
                for (const from of states)
                    for (const to of states)
                        for (const progress of [0, 0.25, 0.5, 0.75, 1]) {
                            const weights = from.map(
                                (weight, i) => weight * (1 - progress) + (to[i] ?? 0) * progress,
                            );
                            const frame = blendLayout({ width, height: 780 }, subpage, weights);
                            expect(frame.left).toBeGreaterThan(0);
                            expect(frame.chat).toBeGreaterThan(0);
                            expect(frame.right).toBeGreaterThanOrEqual(0);
                            expect(frame.offset).toBeLessThanOrEqual(0);
                            expect(
                                frame.offset + frame.left + frame.chat + frame.right,
                            ).toBeGreaterThanOrEqual(Math.max(360, width) - 0.001);
                        }
        },
    );

    it('follows the current width at the same progress and finishes at the exact target', () => {
        const from = layoutWeights('wide', 1);
        const to = layoutWeights('split', 1);
        const weights = from.map((weight, i) => (weight + (to[i] ?? 0)) / 2);
        const first = blendLayout({ width: 1100, height: 780 }, true, weights);
        const next = blendLayout({ width: 1000, height: 780 }, true, weights);
        expect(next.left + next.chat).toBeLessThan(first.left + first.chat);
        const target = responsiveLayout({ width: 1000, height: 780 }, 1, true);
        expect(blendLayout({ width: 1000, height: 780 }, true, to)).toEqual({
            left: target.left,
            chat: target.chat,
            right: target.right,
            offset: target.offset,
        });
    });
});
