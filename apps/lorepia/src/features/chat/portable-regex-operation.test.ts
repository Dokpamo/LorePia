import { describe, expect, it } from 'vitest';

import { performPortableRegexOperation } from './portable-regex-operation';

describe('portable regex worker operation', () => {
    it('compiles compatible JavaScript features without executing them', () => {
        expect(
            performPortableRegexOperation({
                operation: 'compile',
                pattern: '(?<=prefix)(?<value>item)\\1?',
                flags: 'u',
            }),
        ).toEqual({ ok: true, value: true });
    });

    it('reports malformed patterns without affecting later rules', () => {
        expect(
            performPortableRegexOperation({ operation: 'compile', pattern: '(', flags: '' }),
        ).toEqual({ ok: false, reason: 'invalid_pattern' });
        expect(
            performPortableRegexOperation({
                operation: 'replace',
                source: 'hello',
                pattern: '(h)(ello)',
                flags: '',
                replacement: '$2, $1',
            }),
        ).toEqual({ ok: true, value: 'ello, h' });
    });
});
