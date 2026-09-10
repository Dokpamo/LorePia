import { describe, expect, it } from 'vitest';
import { styleRules } from '../tests/css-rules';
import css from './app-css';

const rules = styleRules(css);

describe('input accessibility styling', () => {
    it('retains a visible keyboard-focus indicator', () => {
        const focusRules = rules.filter((rule) => rule.selector.includes(':focus-visible'));
        expect(focusRules.length).toBeGreaterThan(0);
        expect(
            focusRules.some(
                ({ declarations }) =>
                    declarations.outline !== undefined &&
                    !['none', '0'].includes(declarations.outline),
            ),
        ).toBe(true);
    });

    it('restricts pointer hover feedback to devices that support hover', () => {
        const hoverRules = rules.filter((rule) => rule.selector.includes(':hover'));
        expect(hoverRules.length).toBeGreaterThan(0);
        for (const rule of hoverRules) {
            expect(
                rule.media.some(
                    (query) => query.includes('hover: hover') && query.includes('pointer: fine'),
                ),
                rule.selector,
            ).toBe(true);
        }
    });

    it('provides a reduced-motion override without fixing animation durations', () => {
        const reducedMotionRules = rules.filter((rule) =>
            rule.media.some((query) => query.includes('prefers-reduced-motion: reduce')),
        );
        expect(
            reducedMotionRules.some((rule) =>
                Object.keys(rule.declarations).some(
                    (property) =>
                        property.startsWith('transition') || property.startsWith('animation'),
                ),
            ),
        ).toBe(true);
    });
});
