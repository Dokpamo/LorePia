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
    ])('fills all of the %i×%i viewport without transforming page coordinates', (width, height) => {
        const layout = responsiveLayout({ width, height }, 1, true);
        expect(layout.width * layout.scale).toBeCloseTo(width);
        expect(layout.height * layout.scale).toBeCloseTo(height);
        expect(layout.scale).toBe(1);
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

    it('keeps compact density bounded and independent of height or desktop growth', () => {
        const density = (width: number, height = 780) =>
            responsiveLayout({ width, height }, 1, true).density;
        expect(density(320)).toBe(0.9);
        expect(density(360)).toBeCloseTo(360 / 393);
        expect(density(393)).toBe(1);
        expect(density(430)).toBe(1);
        expect(density(1920)).toBe(1);
        expect(density(320, 480)).toBe(density(320, 1000));
        expect(density(240)).toBe(0.9);
        for (let width = 321; width < 450; width++) {
            expect(density(width)).toBeGreaterThanOrEqual(density(width - 1));
            expect(density(width) - density(width - 1)).toBeLessThan(0.003);
        }
    });

    it('uses available width below 360px without introducing letterboxing', () => {
        const smaller = responsiveLayout({ width: 320, height: 640 }, 1, true);
        expect(smaller.width).toBe(320);
        expect(smaller.height).toBe(640);
        expect(smaller.left).toBe(320);
        expect(smaller.chat).toBe(320);
        expect(smaller.offset).toBe(-320);
        expect(smaller.scale).toBe(1);
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
                    expect(layout.chat).toBeGreaterThanOrEqual(Math.min(360, width));
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
