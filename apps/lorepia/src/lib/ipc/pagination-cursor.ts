import type { ReadPageCursor } from './contracts/pagination';

/** Preserve sub-millisecond precision from Rust's UTC timestamps. */
function timestampKey(value: string): readonly [number, string] | null {
    const millis = Date.parse(value);
    if (!Number.isFinite(millis)) return null;
    const fraction = /\.(\d{1,9})(?:Z|[+-]\d{2}:\d{2})$/.exec(value)?.[1] ?? '';
    return [millis, fraction.padEnd(9, '0').slice(3)];
}

export function creatorCursorAdvances(before: ReadPageCursor, after: ReadPageCursor): boolean {
    if (before.scope !== after.scope) return false;
    // Retained for preview/older clients that still return ID-only continuations.
    if (before.after_updated_at === undefined && after.after_updated_at === undefined)
        return after.after_id > before.after_id;
    if (before.after_updated_at === undefined || after.after_updated_at === undefined) return false;
    const previous = timestampKey(before.after_updated_at);
    const next = timestampKey(after.after_updated_at);
    if (previous === null || next === null) return false;
    if (next[0] !== previous[0]) return next[0] < previous[0];
    if (next[1] !== previous[1]) return next[1] < previous[1];
    return after.after_id > before.after_id;
}
