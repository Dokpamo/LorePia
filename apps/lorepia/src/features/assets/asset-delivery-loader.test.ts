import { afterEach, expect, it, vi } from 'vitest';
import type {
    AssetDeliveryDto,
    AssetDeliverySelector,
    LorepiaClient,
} from '../../lib/ipc/contracts';
import { deferred } from '../../tests/deferred';
import { loadAssetDelivery } from './asset-delivery-loader';
import { LorepiaClientError } from '../../lib/ipc/errors';

afterEach(() => vi.useRealTimers());
const value: AssetDeliveryDto = {
    asset_id: 'image',
    sha256: 'ab'.repeat(32),
    url: `lorepia-asset://sha256/${'ab'.repeat(32)}`,
    kind: 'image',
    media_type: 'image/png',
    size_bytes: 200,
    width: 20,
    height: 20,
    duration_ms: null,
};
const selector = { kind: 'asset_id', asset_id: 'image' } as const;

it('measures each queued priority once when filling four available native slots', async () => {
    const finish = deferred<AssetDeliveryDto>();
    const client = {
        resolveAssetDelivery: vi.fn<LorepiaClient['resolveAssetDelivery']>(() => finish.promise),
    };
    const abort = new AbortController();
    const priorities = Array.from({ length: 200 }, (_, index) => vi.fn(() => 200 - index));
    const results = Promise.allSettled(
        priorities.map((priority, index) =>
            loadAssetDelivery(
                client,
                { kind: 'asset_id', asset_id: String(index) },
                abort.signal,
                undefined,
                priority,
            ),
        ),
    );
    priorities.forEach((priority) => priority.mockClear());
    await Promise.resolve();
    expect(client.resolveAssetDelivery).toHaveBeenCalledTimes(4);
    expect(priorities.every((priority) => priority.mock.calls.length === 1)).toBe(true);
    expect(client.resolveAssetDelivery.mock.calls.map(([request]) => request.selector)).toEqual(
        [199, 198, 197, 196].map((index) => ({ kind: 'asset_id', asset_id: String(index) })),
    );
    abort.abort();
    finish.resolve(value);
    await results;
});

it('skips a queued consumer cancelled synchronously by another start without losing a slot', async () => {
    const finish = deferred<AssetDeliveryDto>();
    const aborts = Array.from({ length: 5 }, () => new AbortController());
    const client = {
        resolveAssetDelivery: vi.fn<LorepiaClient['resolveAssetDelivery']>((request) => {
            if (request.selector.kind === 'asset_id' && request.selector.asset_id === '0')
                aborts[1]?.abort();
            return finish.promise;
        }),
    };
    const results = Promise.allSettled(
        aborts.map((abort, index) =>
            loadAssetDelivery(
                client,
                { kind: 'asset_id', asset_id: String(index) },
                abort.signal,
                undefined,
                () => 100,
            ),
        ),
    );
    await Promise.resolve();
    expect(client.resolveAssetDelivery.mock.calls.map(([request]) => request.selector)).toEqual(
        [0, 2, 3, 4].map((index) => ({ kind: 'asset_id', asset_id: String(index) })),
    );
    finish.resolve(value);
    const settled = await results;
    expect(settled.filter((result) => result.status === 'fulfilled')).toHaveLength(4);
    expect(settled[1]?.status).toBe('rejected');
});

it('reconsiders a batch when starting native work synchronously adds a foreground request', async () => {
    const finish = deferred<AssetDeliveryDto>();
    const abort = new AbortController();
    let foreground: Promise<AssetDeliveryDto> | undefined;
    const client = {
        resolveAssetDelivery: vi.fn<LorepiaClient['resolveAssetDelivery']>((request) => {
            if (request.selector.kind === 'asset_id' && request.selector.asset_id === 'A') {
                foreground = loadAssetDelivery(
                    client,
                    { kind: 'asset_id', asset_id: 'visible' },
                    abort.signal,
                );
            }
            return finish.promise;
        }),
    };
    const results = ['A', 'B', 'C', 'D'].map((asset_id) =>
        loadAssetDelivery(
            client,
            { kind: 'asset_id', asset_id },
            abort.signal,
            undefined,
            () => 100,
        ),
    );
    await Promise.resolve();
    expect(client.resolveAssetDelivery.mock.calls.map(([request]) => request.selector)).toEqual(
        ['A', 'visible', 'B', 'C'].map((asset_id) => ({ kind: 'asset_id', asset_id })),
    );
    finish.resolve(value);
    await Promise.all(results);
    await foreground;
});

it('bounds concurrent verification and removes unseen thumbnails from the waiting queue', async () => {
    const requests: ReturnType<typeof deferred<AssetDeliveryDto>>[] = [];
    const client = {
        resolveAssetDelivery: vi.fn(() => {
            const request = deferred<AssetDeliveryDto>();
            requests.push(request);
            return request.promise;
        }),
    };
    const aborts = Array.from({ length: 10 }, () => new AbortController());
    const results = Promise.allSettled(
        aborts.map((abort) => loadAssetDelivery(client, selector, abort.signal)),
    );
    expect(requests).toHaveLength(4);
    aborts[6]?.abort();
    for (let i = 0; i < 9; i++) {
        expect(requests.length - i).toBeLessThanOrEqual(4);
        requests[i]?.resolve(value);
        await vi.waitFor(() => expect(requests.length).toBe(Math.min(9, i + 5)));
    }
    const settled = await results;
    expect(settled.filter((result) => result.status === 'fulfilled')).toHaveLength(9);
    expect(settled[6]?.status).toBe('rejected');
    expect(client.resolveAssetDelivery).toHaveBeenCalledTimes(9);
});

it('recovers as native verification work refills without changing the selector', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const client = {
        resolveAssetDelivery: vi.fn(() => {
            return Date.now() < 8_000
                ? Promise.reject(
                      new LorepiaClientError({
                          code: 'storage_unavailable',
                          message_key: 'error.storage_unavailable',
                          recoverable: true,
                          operation_id: null,
                          field_errors: [],
                      }),
                  )
                : Promise.resolve(value);
        }),
    };
    const result = loadAssetDelivery(client, selector, new AbortController().signal);
    await vi.advanceTimersByTimeAsync(10_000);
    await expect(result).resolves.toEqual(value);
    expect(client.resolveAssetDelivery).toHaveBeenLastCalledWith({ selector });
});

it('lets newly visible images overtake queued preloads using the latest scroll position', async () => {
    const requests: ReturnType<typeof deferred<AssetDeliveryDto>>[] = [];
    const client = {
        resolveAssetDelivery: vi.fn<LorepiaClient['resolveAssetDelivery']>(() => {
            const request = deferred<AssetDeliveryDto>();
            requests.push(request);
            return request.promise;
        }),
    };
    const abort = new AbortController();
    const results = Array.from({ length: 4 }, () =>
        loadAssetDelivery(client, selector, abort.signal),
    );
    let nextDistance = 600;
    let oldDistance = 0;
    for (const [id, priority] of [
        ['far', () => 2000],
        ['next', () => nextDistance],
        ['old', () => oldDistance],
    ] as const) {
        results.push(
            loadAssetDelivery(
                client,
                { kind: 'asset_id', asset_id: id },
                abort.signal,
                undefined,
                priority,
            ),
        );
    }
    // The user scrolls while all four real native calls are still active.
    nextDistance = 0;
    oldDistance = 800;
    requests.slice(0, 4).forEach((request) => request.resolve(value));
    await vi.waitFor(() => expect(requests).toHaveLength(7));
    expect(
        client.resolveAssetDelivery.mock.calls.slice(4).map(([request]) => request.selector),
    ).toEqual([
        { kind: 'asset_id', asset_id: 'next' },
        { kind: 'asset_id', asset_id: 'old' },
        { kind: 'asset_id', asset_id: 'far' },
    ]);
    requests.slice(4).forEach((request) => request.resolve(value));
    await Promise.all(results);
});

it('reserves the first slots for visible images even when this render mounts preloads first', async () => {
    const finish = deferred<AssetDeliveryDto>();
    const client = {
        resolveAssetDelivery: vi.fn<LorepiaClient['resolveAssetDelivery']>(() => finish.promise),
    };
    const signal = new AbortController().signal;
    const background = loadAssetDelivery(
        client,
        { kind: 'asset_id', asset_id: 'background' },
        signal,
        undefined,
        () => 900,
    );
    const foreground = loadAssetDelivery(
        client,
        { kind: 'asset_id', asset_id: 'foreground' },
        signal,
    );
    expect(client.resolveAssetDelivery.mock.calls.map(([request]) => request.selector)).toEqual([
        { kind: 'asset_id', asset_id: 'foreground' },
        { kind: 'asset_id', asset_id: 'background' },
    ]);
    finish.resolve(value);
    await Promise.all([background, foreground]);
});

it('retries immediately when the user returns after background timers were suspended', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const client = {
        resolveAssetDelivery: vi
            .fn()
            .mockRejectedValueOnce({ code: 'storage_unavailable', recoverable: true })
            .mockResolvedValue(value),
    };
    const result = loadAssetDelivery(client, selector, new AbortController().signal);
    await vi.advanceTimersByTimeAsync(0);
    expect(client.resolveAssetDelivery).toHaveBeenCalledOnce();
    // Wall time advances while the WebView's timeout callbacks are suspended.
    vi.setSystemTime(90_000);
    window.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(0);
    expect(client.resolveAssetDelivery).toHaveBeenCalledTimes(2);
    await expect(result).resolves.toEqual(value);
});

it('ends a hung request without allowing more than four native calls in flight', async () => {
    vi.useFakeTimers();
    const requests: ReturnType<typeof deferred<AssetDeliveryDto>>[] = [];
    const client = {
        resolveAssetDelivery: vi.fn(() => {
            const request = deferred<AssetDeliveryDto>();
            requests.push(request);
            return request.promise;
        }),
    };
    const results = Promise.allSettled(
        Array.from({ length: 5 }, () =>
            loadAssetDelivery(client, selector, new AbortController().signal),
        ),
    );
    expect(requests).toHaveLength(4);
    await vi.advanceTimersByTimeAsync(30_001);
    expect(vi.getTimerCount()).toBe(0);
    // Release native work only after checking that the fifth call did not start.
    expect(requests).toHaveLength(4);
    requests.forEach((request) => request.resolve(value));
    await vi.advanceTimersByTimeAsync(0);
    const settled = await results;
    expect(
        settled.every(
            (result) =>
                result.status === 'rejected' &&
                result.reason instanceof DOMException &&
                result.reason.name === 'TimeoutError',
        ),
    ).toBe(true);
    expect(client.resolveAssetDelivery).toHaveBeenCalledTimes(4);
});

it('lets an already-available image load while four other images wait to retry', async () => {
    vi.useFakeTimers();
    const unavailable = new LorepiaClientError({
        code: 'storage_unavailable',
        message_key: 'error.storage_unavailable',
        recoverable: true,
        operation_id: null,
        field_errors: [],
    });
    const client = {
        resolveAssetDelivery: vi.fn(({ selector: request }: { selector: AssetDeliverySelector }) =>
            request.kind === 'asset_id' && request.asset_id === 'available'
                ? Promise.resolve(value)
                : Promise.reject(unavailable),
        ),
    };
    const aborts = Array.from({ length: 5 }, () => new AbortController());
    const result = Promise.allSettled(
        aborts.map((abort, index) =>
            loadAssetDelivery(
                client,
                {
                    kind: 'asset_id',
                    asset_id: index === 4 ? 'available' : `waiting-${String(index)}`,
                },
                abort.signal,
            ),
        ),
    );
    try {
        await vi.advanceTimersByTimeAsync(0);
        expect(client.resolveAssetDelivery).toHaveBeenCalledTimes(5);
    } finally {
        aborts.forEach((abort) => abort.abort());
        await result;
    }
});

it('stops retrying corrupt assets and cancels retry waits when a thumbnail leaves view', async () => {
    vi.useFakeTimers();
    const bad = { code: 'storage_corrupted', recoverable: false };
    const broken = { resolveAssetDelivery: vi.fn().mockRejectedValue(bad) };
    await expect(
        loadAssetDelivery(broken, selector, new AbortController().signal),
    ).rejects.toMatchObject(bad);
    expect(broken.resolveAssetDelivery).toHaveBeenCalledOnce();
    const client = {
        resolveAssetDelivery: vi
            .fn()
            .mockRejectedValue({ code: 'storage_unavailable', recoverable: true }),
    };
    const abort = new AbortController();
    const result = loadAssetDelivery(client, selector, abort.signal);
    const rejected = expect(result).rejects.toMatchObject({ name: 'AbortError' });
    await vi.advanceTimersByTimeAsync(1);
    abort.abort();
    await rejected;
    await vi.advanceTimersByTimeAsync(80_000);
    expect(client.resolveAssetDelivery).toHaveBeenCalledOnce();
});
