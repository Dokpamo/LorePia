import { describe, expect, it } from 'vitest';
import { chatActivityTime } from '../../ui/navigation/chat-activity';

describe('Korean chat activity dates', () => {
    it.each([
        [2026, 'ko', '9월 9일'],
        [2025, 'ko', '25년 9월 9일'],
        [2027, 'ko', '27년 9월 9일'],
        [2025, 'ko-KR', '25년 9월 9일'],
    ])('formats a %i chat date in %s as %s', (year, locale, expected) => {
        const time = chatActivityTime(
            new Date(year, 8, 9, 13, 4).toISOString(),
            locale,
            new Date(2026, 8, 10),
        );
        expect(time?.short).toBe(expected);
        expect(time?.full).toContain(`${String(year)}년`);
    });
    it('uses the local calendar year across New Year and keeps time for today', () => {
        const now = new Date(2026, 0, 1, 15, 0);
        expect(
            chatActivityTime(new Date(2025, 11, 31, 23, 59).toISOString(), 'ko', now)?.short,
        ).toBe('25년 12월 31일');
        expect(chatActivityTime(new Date(2026, 0, 1, 13, 4).toISOString(), 'ko', now)?.short).toBe(
            '오후 1:04',
        );
    });
});
