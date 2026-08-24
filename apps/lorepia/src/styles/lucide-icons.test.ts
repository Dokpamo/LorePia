import { describe, expect, it } from 'vitest';

const svelteSources = import.meta.glob<string>('../**/*.svelte', {
    eager: true,
    import: 'default',
    query: '?raw',
});

describe('Lucide icon contract', () => {
    it('keeps hand-authored inline SVG out of application components', () => {
        expect(Object.keys(svelteSources).length).toBeGreaterThan(0);

        for (const [path, source] of Object.entries(svelteSources)) {
            expect(source, path).not.toMatch(/<svg\b/i);
        }
    });
});
