import { describe, expect, it } from 'vitest';
import { applyPortableFrameLayout, portableRegionPath } from './portable-frame-layout';
import { isPortableRendererMessage, PORTABLE_RENDERER_CHANNEL } from './portable-renderer-protocol';

const identity = { channel: PORTABLE_RENDERER_CHANNEL, runtimeId: 'frame' } as const;
describe('portable frame layout', () => {
    it('grows and shrinks a long message in the transcript without a viewport-height clamp', () => {
        const frame = document.createElement('iframe');
        for (const height of [9000, 80]) {
            const message = { ...identity, type: 'portable_resize', height };
            expect(isPortableRendererMessage(message, 'frame')).toBe(true);
            if (!isPortableRendererMessage(message, 'frame')) throw new Error('Invalid fixture');
            applyPortableFrameLayout(frame, message, false, false);
            expect(frame.style.height).toBe(`${String(height)}px`);
        }
        for (const height of [-1, 65_537, Infinity, NaN, 200.5])
            expect(
                isPortableRendererMessage(
                    { ...identity, type: 'portable_resize', height },
                    'frame',
                ),
            ).toBe(false);
    });
    it('bounds card hit regions to its viewport and leaves empty space noninteractive', () => {
        expect(portableRegionPath([], 393, 700)).toBe('path("M0,0Z")');
        expect(portableRegionPath([{ x: 340, y: 650, width: 200, height: 200 }], 393, 700)).toBe(
            'path("M340,650H393V700H340Z")',
        );
        const frame = document.createElement('iframe');
        Object.defineProperties(frame, {
            clientWidth: { value: 393 },
            clientHeight: { value: 700 },
        });
        const message = {
            ...identity,
            type: 'portable_regions',
            regions: [{ x: 340, y: 20, width: 40, height: 40 }],
        } as const;
        expect(isPortableRendererMessage(message, 'frame')).toBe(true);
        if (!isPortableRendererMessage(message, 'frame')) throw new Error('Invalid fixture');
        applyPortableFrameLayout(frame, message, false, true);
        expect(frame.style.clipPath).toBe('');
        applyPortableFrameLayout(frame, message, true, false);
        expect(frame.style.clipPath).toBe('');
        applyPortableFrameLayout(frame, message, true, true);
        expect(frame.style.clipPath).toBe('path("M340,20H380V60H340Z")');
    });
    it('rejects malformed, oversized or foreign layout reports', () => {
        const region = { x: 1, y: 2, width: 30, height: 40 };
        for (const regions of [
            [{ ...region, x: -1 }],
            [{ ...region, y: Infinity }],
            [{ ...region, width: '40' }],
            [{ ...region, height: 65_537 }],
            [null],
            Array.from({ length: 129 }, () => region),
        ])
            expect(
                isPortableRendererMessage(
                    { ...identity, type: 'portable_regions', regions },
                    'frame',
                ),
            ).toBe(false);
        expect(
            isPortableRendererMessage(
                { ...identity, type: 'portable_regions', regions: [region] },
                'other',
            ),
        ).toBe(false);
    });
});
