import { describe, expect, it } from 'vitest';

import { characterDescriptionPreview } from './character-preview';

describe('characterDescriptionPreview', () => {
    it('keeps a useful heading while hiding imported template source', () => {
        expect(
            characterDescriptionPreview(
                '## Simulation: Alternate Hunters {{#if_pure {{getvar::lang}}::1}}localized',
            ),
        ).toBe('Simulation: Alternate Hunters');
    });

    it('returns an empty fallback when the description starts with a template', () => {
        expect(characterDescriptionPreview('{{user}} private character')).toBe('');
    });
});
