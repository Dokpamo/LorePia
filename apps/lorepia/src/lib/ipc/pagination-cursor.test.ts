import { expect, it } from 'vitest';
import { creatorCursorAdvances } from './pagination-cursor';

const cursor = (id: string, time: string, scope = 'collection') => ({
    scope,
    after_id: id,
    after_updated_at: time,
});

it('compares recency with nanosecond precision and ascending IDs for timestamp ties', () => {
    const before = cursor('z', '2026-09-10T00:00:00.123456789Z');
    expect(creatorCursorAdvances(before, cursor('a', '2026-09-10T00:00:00.123456788Z'))).toBe(true);
    expect(creatorCursorAdvances(before, cursor('a', '2026-09-10T00:00:00.123456790Z'))).toBe(
        false,
    );
    expect(creatorCursorAdvances(cursor('a', before.after_updated_at), before)).toBe(true);
    expect(creatorCursorAdvances(before, before)).toBe(false);
    expect(creatorCursorAdvances(before, cursor('zz', before.after_updated_at, 'other'))).toBe(
        false,
    );
    expect(creatorCursorAdvances(before, cursor('zz', 'invalid'))).toBe(false);
});
