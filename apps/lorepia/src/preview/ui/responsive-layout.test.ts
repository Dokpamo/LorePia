import { describe, expect, it } from 'vitest';
import { responsiveLayout } from './responsive-layout';
import type { Page } from './sample-data';

describe('responsive connected pages', () => {
    it.each([
        [320, 640],
        [360, 780],
        [393, 748],
        [430, 932],
        [480, 700],
        [600, 500],
    ])('fills all of the %i×%i viewport while scaling controls uniformly', (width, height) => {
        const layout = responsiveLayout({ width, height }, 1, true);
        expect(layout.width * layout.scale).toBeCloseTo(width);
        expect(layout.height * layout.scale).toBeCloseTo(height);
    });

    it.each([
        [359, 'mobile'],
        [360, 'mobile'],
        [759, 'mobile'],
        [760, 'split'],
        [1119, 'split'],
        [1120, 'wide'],
        [1600, 'wide'],
    ] as const)('uses the intended composition at %ipx', (width, mode) => {
        expect(responsiveLayout({ width, height: 780 }, 1, true).mode).toBe(mode);
    });

    it('keeps the same logical layout below 360px and fills the physical viewport', () => {
        const reference = responsiveLayout({ width: 360, height: 720 }, 1, true);
        const smaller = responsiveLayout({ width: 320, height: 640 }, 1, true);
        expect(smaller.width).toBe(reference.width);
        expect(smaller.height).toBe(reference.height);
        expect(smaller.left).toBe(reference.left);
        expect(smaller.chat).toBe(reference.chat);
        expect(smaller.offset).toBe(reference.offset);
        expect(smaller.width * smaller.scale).toBe(320);
        expect(smaller.height * smaller.scale).toBe(640);
        expect(responsiveLayout({ width: 393, height: 780 }, 1, true).scale).toBe(1);
    });

    it.each([320, 360, 393, 759, 760, 900, 1119, 1120, 1440, 1920])(
        'covers the viewport without gaps, overlap or a cropped active page at %ipx',
        (width) => {
            for (const subpage of [false, true]) {
                for (const page of [0, 1, 2] as Page[]) {
                    const layout = responsiveLayout({ width, height: 780 }, page, subpage);
                    const widths = [layout.left, layout.chat, layout.right] as const;
                    const positions = [0, layout.left, layout.left + layout.chat] as const;
                    expect(layout.visible).toContain(page === 2 && !subpage ? 1 : page);
                    const first = layout.visible[0];
                    const last = layout.visible.at(-1);
                    if (first === undefined || last === undefined)
                        throw new Error('No visible page');
                    expect(positions[first] + layout.offset).toBeCloseTo(0);
                    expect(positions[last] + widths[last] + layout.offset).toBeCloseTo(
                        layout.width,
                    );
                    expect(layout.chat).toBeGreaterThanOrEqual(360);
                    if (!subpage) {
                        expect(layout.visible).not.toContain(2);
                        expect(layout.right).toBe(0);
                    }
                }
            }
        },
    );

    it('gives space to chat while keeping desktop side columns within readable bounds', () => {
        const wide = responsiveLayout({ width: 1920, height: 1000 }, 1, true);
        expect(wide.left).toBeLessThanOrEqual(360);
        expect(wide.right).toBeLessThanOrEqual(320);
        expect(wide.chat).toBeGreaterThan(1000);
        const split = responsiveLayout({ width: 800, height: 780 }, 2, true);
        expect(split.visible).toEqual([1, 2]);
        expect(split.chat).toBeGreaterThan(split.right);
        expect(split.offset).toBe(-split.left);
        expect(split.chat).toBe(responsiveLayout({ width: 800, height: 780 }, 0, true).chat);
    });
});
