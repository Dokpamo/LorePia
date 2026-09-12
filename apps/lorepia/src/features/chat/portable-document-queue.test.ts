import { expect, it, vi } from 'vitest';
import { PortableDocumentQueue } from './portable-document-queue';
import { deferred } from '../../tests/deferred';

it('keeps only one running build and the latest of 200 replacements', async () => {
    const queue = new PortableDocumentQueue();
    const first = deferred<{ content: string; style: string }>();
    const publish = vi.fn();
    const initial = vi.fn(() => first.promise);
    queue.submit(['frame'], initial, publish);
    const replacements = Array.from({ length: 200 }, (_, index) =>
        vi.fn(() => Promise.resolve({ content: String(index), style: '' })),
    );
    for (const build of replacements) queue.submit(['frame'], build, publish);
    expect(initial).toHaveBeenCalledOnce();
    expect(replacements.every((build) => build.mock.calls.length === 0)).toBe(true);
    first.resolve({ content: 'old', style: '' });
    await vi.waitFor(() => expect(publish).toHaveBeenCalledOnce());
    expect(publish.mock.calls[0]?.[0]).toEqual({ content: '199', style: '' });
    expect(replacements.slice(0, -1).every((build) => build.mock.calls.length === 0)).toBe(true);
    queue.clear();
});
it('retains bridge identity for unchanged documents and rotates it for changed scope or content', async () => {
    const queue = new PortableDocumentQueue();
    const publish = vi.fn();
    const result = { content: 'safe', style: '' };
    queue.submit(['scope'], () => Promise.resolve(result), publish);
    await vi.waitFor(() => expect(publish).toHaveBeenCalledOnce());
    const id = queue.activeRuntimeId;
    expect(queue.accepts(id)).toBe(true);
    queue.submit(['scope'], () => Promise.resolve(result), publish);
    expect(queue.accepts(id)).toBe(false);
    await vi.waitFor(() => expect(publish).toHaveBeenCalledTimes(2));
    expect(publish.mock.calls[1]?.slice(1)).toEqual([id, false]);
    expect(queue.accepts(id)).toBe(true);
    queue.submit(['other-scope'], () => Promise.resolve(result), publish);
    await vi.waitFor(() => expect(publish).toHaveBeenCalledTimes(3));
    expect(queue.activeRuntimeId).not.toBe(id);
    expect(queue.accepts(id)).toBe(false);
    queue.clear();
});
it('clears pending work and never publishes after teardown', async () => {
    const queue = new PortableDocumentQueue();
    const delayed = deferred<{ content: string; style: string }>();
    const publish = vi.fn();
    queue.submit([], () => delayed.promise, publish);
    const replacement = vi.fn(() => Promise.resolve({ content: 'new', style: '' }));
    queue.submit([], replacement, publish);
    queue.clear();
    delayed.resolve({ content: 'old', style: '' });
    await delayed.promise;
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(replacement).not.toHaveBeenCalled();
    expect(publish).not.toHaveBeenCalled();
});

it('recovers from a synchronous build failure without publishing it', async () => {
    const queue = new PortableDocumentQueue();
    const publish = vi.fn();
    queue.submit(
        [],
        () => {
            throw new Error('failed');
        },
        publish,
    );
    queue.submit([], () => Promise.resolve({ content: 'latest', style: '' }), publish);
    await vi.waitFor(() => expect(publish).toHaveBeenCalledOnce());
    expect(publish.mock.calls[0]?.[0]).toEqual({ content: 'latest', style: '' });
    queue.clear();
});
