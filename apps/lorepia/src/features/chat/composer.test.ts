import { describe, expect, it } from 'vitest';

import { shouldSubmitComposer } from './composer';

describe('shouldSubmitComposer', () => {
    it('does not submit Korean, Japanese or Chinese composition on Enter', () => {
        for (const sample of ['안녕', 'こんにちは', '你好']) {
            expect(sample).not.toHaveLength(0);
            expect(
                shouldSubmitComposer({ key: 'Enter', shiftKey: false, isComposing: true }, true),
            ).toBe(false);
        }
    });

    it('uses Shift+Enter for a newline and plain Enter for send', () => {
        expect(
            shouldSubmitComposer({ key: 'Enter', shiftKey: true, isComposing: false }, false),
        ).toBe(false);
        expect(
            shouldSubmitComposer({ key: 'Enter', shiftKey: false, isComposing: false }, false),
        ).toBe(true);
    });
});
