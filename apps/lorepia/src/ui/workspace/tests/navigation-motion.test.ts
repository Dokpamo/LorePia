import { describe, expect, it } from 'vitest';
import { navigationTiming } from '../navigation-motion';

describe('navigation settling', () => {
    it.each([0, 0.4, 2, 8])('lands without exposing a window edge at velocity %s', (velocity) => {
        const { easing, duration } = navigationTiming(240, velocity);
        const positions = Array.from({ length: 101 }, (_, index) => easing(index / 100));
        expect(positions[0]).toBe(0);
        expect(positions.at(-1)).toBe(1);
        expect(
            positions.every((value, index) => value >= (positions[index - 1] ?? 0) && value <= 1),
        ).toBe(true);
        expect(duration).toBeGreaterThanOrEqual(200);
        expect(duration).toBeLessThanOrEqual(420);
    });
    it('carries release speed into the initial motion instead of starting at rest again', () => {
        const slow = navigationTiming(200);
        const fast = navigationTiming(200, 1.2);
        expect(fast.easing(0.04)).toBeGreaterThan(slow.easing(0.04));
        expect(fast.duration).toBeLessThan(slow.duration);
    });
});
