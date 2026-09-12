import { afterEach, expect, it, vi } from 'vitest';
import { releaseThumbnail, retainThumbnail } from './thumbnail-retention';

const elements: HTMLElement[] = [];
const MiB = 1024 * 1024;
function element() {
    const value = document.createElement('span');
    elements.push(value);
    return value;
}
afterEach(() => {
    elements.forEach(releaseThumbnail);
    elements.length = 0;
    vi.useRealTimers();
});

it('releases idle offscreen decoded media after 30 seconds with one shared timer', () => {
    vi.useFakeTimers();
    const first = vi.fn();
    const second = vi.fn();
    retainThumbnail(element(), 24 * MiB, first);
    vi.advanceTimersByTime(10_000);
    retainThumbnail(element(), 24 * MiB, second);
    expect(vi.getTimerCount()).toBe(1);
    vi.advanceTimersByTime(19_999);
    expect(first).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(first).toHaveBeenCalledOnce();
    expect(second).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(1);
    vi.advanceTimersByTime(10_000);
    expect(second).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
});

it('protects returned images and grants a fresh retention window on the next departure', () => {
    vi.useFakeTimers();
    const image = element();
    const evict = vi.fn();
    retainThumbnail(image, MiB, evict);
    vi.advanceTimersByTime(20_000);
    releaseThumbnail(image);
    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(20_000);
    expect(evict).not.toHaveBeenCalled();
    retainThumbnail(image, MiB, evict);
    vi.advanceTimersByTime(29_999);
    expect(evict).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(evict).toHaveBeenCalledOnce();
});

it('evicts older decoded media at the memory bound and refreshes revisited media', () => {
    const first = element();
    const second = element();
    const third = element();
    const evictFirst = vi.fn();
    const evictSecond = vi.fn();
    const evictThird = vi.fn();
    retainThumbnail(first, 24 * MiB, evictFirst);
    retainThumbnail(second, 24 * MiB, evictSecond);
    releaseThumbnail(first); // Back in view, then recently left again.
    retainThumbnail(first, 24 * MiB, evictFirst);
    retainThumbnail(third, 24 * MiB, evictThird);
    expect(evictSecond).toHaveBeenCalledOnce();
    expect(evictFirst).not.toHaveBeenCalled();
    expect(evictThird).not.toHaveBeenCalled();
    releaseThumbnail(first);
    releaseThumbnail(first); // Duplicate teardown must not subtract twice.
    retainThumbnail(element(), 40 * MiB, vi.fn());
    expect(evictThird).not.toHaveBeenCalled();
    retainThumbnail(element(), 1, vi.fn());
    expect(evictThird).toHaveBeenCalledOnce();
});

it('bounds tiny decoded elements as well as total bytes and rejects an oversized entry', () => {
    const evictions = Array.from({ length: 65 }, () => vi.fn());
    evictions.forEach((evict) => retainThumbnail(element(), 1, evict));
    expect(evictions[0]).toHaveBeenCalledOnce();
    expect(evictions.slice(1).every((evict) => evict.mock.calls.length === 0)).toBe(true);
    const oversize = vi.fn();
    retainThumbnail(element(), 65 * MiB, oversize);
    expect(oversize).toHaveBeenCalledOnce();
    expect(evictions[1]).not.toHaveBeenCalled();
});
