import { get } from 'svelte/store';
import { describe, expect, it } from 'vitest';

import { ko } from './ko';
import { locale, t, tr } from './index';

describe('message catalog', () => {
    it('reads a message by key', () => {
        expect(t('common.refresh')).toBe(ko['common.refresh']);
    });

    it('substitutes named placeholders', () => {
        expect(t('common.refresh')).not.toContain('{');
    });

    it('exposes a reactive reader for components', () => {
        expect(get(tr)('common.refresh')).toBe(ko['common.refresh']);
    });

    it('defaults to Korean', () => {
        expect(get(locale)).toBe('ko');
    });

    it('has no message left containing an unsubstituted placeholder name', () => {
        // Guards against a key whose placeholder was renamed in one place only.
        for (const [key, message] of Object.entries(ko)) {
            expect(message, key).not.toMatch(/\{\s*\}/);
        }
    });
});
