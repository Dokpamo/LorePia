import { afterEach, expect, it, vi } from 'vitest';
import { resolvePortableAssets } from './portable-asset-resolution';
import type { AssetDeliveryDto } from '../../lib/ipc/contracts';
import { deferred } from '../../tests/deferred';
afterEach(() => vi.useRealTimers());
const delivery = (id: string) => ({ asset_id: id, sha256: 'ab'.repeat(32) }) as AssetDeliveryDto;
it('deduplicates 128 references to the same asset within one build, without cross-build caching', async () => {
    const client = { resolveAssetDelivery: vi.fn().mockResolvedValue(delivery('asset')) };
    const references = Array.from({ length: 128 }, (_, index) => String(index));
    const signal = new AbortController().signal;
    const result = await resolvePortableAssets(
        references,
        () => ({ asset_id: 'asset' }),
        client,
        signal,
        (value) => value,
    );
    expect(result.size).toBe(128);
    expect(client.resolveAssetDelivery).toHaveBeenCalledOnce();
    await resolvePortableAssets(
        references,
        () => ({ asset_id: 'asset' }),
        client,
        signal,
        (value) => value,
    );
    expect(client.resolveAssetDelivery).toHaveBeenCalledTimes(2);
});
it('shares four native slots across portable builds and aborts queued work', async () => {
    const calls: ReturnType<typeof deferred<AssetDeliveryDto>>[] = [];
    const client = {
        resolveAssetDelivery: vi.fn(() => {
            const next = deferred<AssetDeliveryDto>();
            calls.push(next);
            return next.promise;
        }),
    };
    const cancel = new AbortController();
    const work = resolvePortableAssets(
        ['a', 'b', 'c', 'd', 'e', 'f'],
        (id) => ({ asset_id: id }),
        client,
        cancel.signal,
        (value) => value,
    );
    const rejected = expect(work).rejects.toMatchObject({ name: 'AbortError' });
    expect(client.resolveAssetDelivery).toHaveBeenCalledTimes(4);
    cancel.abort();
    await rejected;
    for (const call of calls) call.resolve(delivery('a'));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(client.resolveAssetDelivery).toHaveBeenCalledTimes(4);
});
it('retries only recoverable backpressure, with an eight-attempt ceiling and cancellation during waits', async () => {
    vi.useFakeTimers();
    const transient = { code: 'storage_unavailable', recoverable: true, message: 'busy' };
    const client = { resolveAssetDelivery: vi.fn().mockRejectedValue(transient) };
    const work = resolvePortableAssets(
        ['a'],
        (id) => ({ asset_id: id }),
        client,
        new AbortController().signal,
        (value) => value,
    );
    await vi.runAllTimersAsync();
    expect((await work).get('a')).toBeNull();
    expect(client.resolveAssetDelivery).toHaveBeenCalledTimes(8);
    client.resolveAssetDelivery
        .mockReset()
        .mockRejectedValue({ code: 'storage_corrupted', recoverable: false });
    expect(
        (
            await resolvePortableAssets(
                ['a'],
                (id) => ({ asset_id: id }),
                client,
                new AbortController().signal,
                (value) => value,
            )
        ).get('a'),
    ).toBeNull();
    expect(client.resolveAssetDelivery).toHaveBeenCalledOnce();
    client.resolveAssetDelivery.mockReset().mockRejectedValue(transient);
    const cancel = new AbortController();
    const cancelled = resolvePortableAssets(
        ['a'],
        (id) => ({ asset_id: id }),
        client,
        cancel.signal,
        (value) => value,
    );
    const rejection = expect(cancelled).rejects.toMatchObject({ name: 'AbortError' });
    await vi.advanceTimersByTimeAsync(1);
    cancel.abort();
    await rejection;
    await vi.runAllTimersAsync();
    expect(client.resolveAssetDelivery).toHaveBeenCalledOnce();
});
