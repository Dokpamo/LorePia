import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { tick } from 'svelte';
import { createPreviewClient } from '../../preview/mock-client';
import type { AssetDeliveryDto, LorepiaClient } from '../../lib/ipc/contracts';
import ProfileThumbnail from './ProfileThumbnail.svelte';
import ProfileImageAlbum from './ProfileImageAlbum.svelte';

vi.mock('@tauri-apps/api/core', () => ({
    convertFileSrc: (digest: string) => `lorepia-asset://localhost/${digest}`,
}));

afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.unstubAllGlobals();
});

function observeThumbnails() {
    const observers = new Map<Element, IntersectionObserverCallback>();
    vi.stubGlobal(
        'IntersectionObserver',
        class {
            constructor(private callback: IntersectionObserverCallback) {}
            observe = (target: Element) => observers.set(target, this.callback);
            unobserve = (target: Element) => observers.delete(target);
            disconnect = vi.fn();
        },
    );
    return {
        observers,
        tile(index: number) {
            const target = [...observers.keys()][index];
            if (!target) throw new Error('Expected an observed thumbnail');
            return target;
        },
        async visible(target: Element, isIntersecting: boolean) {
            observers.get(target)?.(
                [
                    {
                        target,
                        isIntersecting,
                        boundingClientRect: target.getBoundingClientRect(),
                    } as IntersectionObserverEntry,
                ],
                {} as IntersectionObserver,
            );
            await tick();
        },
    };
}

function descriptor(id = 'sample'): AssetDeliveryDto {
    const sha256 = 'ab'.repeat(32);
    return {
        asset_id: id,
        sha256,
        media_type: 'image/png',
        kind: 'image',
        size_bytes: 2048,
        width: 700,
        height: 700,
        duration_ms: null,
        url: `lorepia-asset://sha256/${sha256}`,
    };
}

it('starts visible thumbnails immediately during scrolling and leaves unseen images unrequested', async () => {
    vi.useFakeTimers();
    const visibility = observeThumbnails();
    const client = createPreviewClient();
    const resolve = vi.fn<LorepiaClient['resolveAssetDelivery']>().mockResolvedValue(descriptor());
    client.resolveAssetDelivery = resolve;
    const view = render(ProfileImageAlbum, {
        client,
        images: Array.from({ length: 278 }, (_, index) => ({
            assetId: `image-${String(index)}`,
            title: `Image ${String(index)}`,
        })),
        onview: vi.fn(),
    });
    await fireEvent.scroll(view.container);
    await visibility.visible(visibility.tile(120), true);
    await visibility.visible(visibility.tile(121), true);
    await fireEvent.scroll(view.container);
    // No timer advances and no scroll-quiet interval are needed to begin work.
    expect(resolve.mock.calls.map(([{ selector }]) => selector)).toEqual([
        { kind: 'asset_id', asset_id: 'image-120' },
        { kind: 'asset_id', asset_id: 'image-121' },
    ]);
    view.unmount();
    await vi.advanceTimersByTimeAsync(0);
    expect(vi.getTimerCount()).toBe(0);
});

it('removes a queued thumbnail that leaves view without sending its native request', async () => {
    const visibility = observeThumbnails();
    const client = createPreviewClient();
    const finish: ((value: AssetDeliveryDto) => void)[] = [];
    const resolve = vi
        .fn<LorepiaClient['resolveAssetDelivery']>()
        .mockImplementation(() => new Promise((done) => finish.push(done)));
    client.resolveAssetDelivery = resolve;
    const view = render(ProfileImageAlbum, {
        client,
        images: Array.from({ length: 5 }, (_, index) => ({
            assetId: `image-${String(index)}`,
            title: `Image ${String(index)}`,
        })),
        onview: vi.fn(),
    });
    const tiles = [...visibility.observers.keys()];
    for (const tile of tiles) await visibility.visible(tile, true);
    expect(resolve).toHaveBeenCalledTimes(4);
    const leaving = visibility.tile(4);
    await visibility.visible(leaving, false);
    expect(leaving.querySelector('.trusted-asset')).toBeNull();
    finish.forEach((done) => done(descriptor()));
    await waitFor(() =>
        expect(view.container.querySelectorAll('[data-asset-phase="error"]')).toHaveLength(4),
    );
    expect(resolve).toHaveBeenCalledTimes(4);
});

it('keeps decoded pixels while scrolling away and back, then invalidates changed identity', async () => {
    const visibility = observeThumbnails();
    const client = createPreviewClient();
    const resolve = vi.fn<LorepiaClient['resolveAssetDelivery']>().mockResolvedValue(descriptor());
    client.resolveAssetDelivery = resolve;
    const view = render(ProfileThumbnail, { client, assetId: 'sample', name: 'Sample' });
    const tile = visibility.tile(0);
    await visibility.visible(tile, true);
    const image = await screen.findByRole('img', { name: 'Sample' });
    Object.defineProperties(image, { naturalWidth: { value: 700 }, naturalHeight: { value: 700 } });
    await fireEvent.load(image);
    await visibility.visible(tile, false);
    expect(tile.querySelector('img')).toBe(image);
    await visibility.visible(tile, true);
    expect(tile.querySelector('img')).toBe(image);
    expect(resolve).toHaveBeenCalledOnce();
    await visibility.visible(tile, false);
    await view.rerender({ assetId: 'changed' });
    expect(tile.querySelector('img')).toBeNull();
    resolve.mockResolvedValue(descriptor('changed'));
    await visibility.visible(tile, true);
    await waitFor(() => expect(resolve).toHaveBeenCalledTimes(2));
    expect(resolve).toHaveBeenLastCalledWith({
        selector: { kind: 'asset_id', asset_id: 'changed' },
    });
});

it('cancels offscreen retry work and retries on a later visit', async () => {
    const visibility = observeThumbnails();
    const client = createPreviewClient();
    const resolve = vi.fn().mockRejectedValue(new Error('Fixture'));
    client.resolveAssetDelivery = resolve;
    const view = render(ProfileThumbnail, { client, assetId: 'sample', name: 'Sample' });
    const tile = visibility.tile(0);
    expect(resolve).not.toHaveBeenCalled();
    await visibility.visible(tile, true);
    await screen.findByRole('alert');
    expect(resolve).toHaveBeenCalledOnce();
    await visibility.visible(tile, false);
    expect(view.container.querySelector('.trusted-asset')).toBeNull();
    await visibility.visible(tile, true);
    await waitFor(() => expect(resolve).toHaveBeenCalledTimes(2));
});
