import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { createPreviewClient } from '../../preview/mock-client';
import { t } from '../../lib/i18n';
import ProfileImageAlbum from './ProfileImageAlbum.svelte';
import ProfileThumbnail from './ProfileThumbnail.svelte';

afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
});

it('lists images as thumbnails and opens the original only when a thumbnail is activated', async () => {
    const client = createPreviewClient();
    client.resolveAssetDelivery = vi.fn().mockRejectedValue(new Error('No test media'));
    const onview = vi.fn();
    render(ProfileImageAlbum, {
        client,
        images: [
            { assetId: 'first', title: 'Default' },
            { assetId: 'second', title: 'Smile' },
        ],
        onview,
    });
    const second = screen.getByRole('button', {
        name: t('navigation.imageSelectItem', { name: 'Smile' }),
    });
    expect(onview).not.toHaveBeenCalled();
    expect(screen.queryByRole('dialog')).toBeNull();
    await fireEvent.click(second);
    expect(onview).toHaveBeenCalledExactlyOnceWith({ assetId: 'second', title: 'Smile' }, second);
});

it('does not resolve offscreen thumbnails and disconnects its observer', async () => {
    let intersect: IntersectionObserverCallback | undefined;
    let target: Element | undefined;
    const disconnect = vi.fn();
    vi.stubGlobal(
        'IntersectionObserver',
        class {
            constructor(callback: IntersectionObserverCallback) {
                intersect = callback;
            }
            observe = (element: Element) => {
                target = element;
            };
            unobserve = vi.fn();
            disconnect = disconnect;
        },
    );
    const client = createPreviewClient();
    const resolve = vi.fn().mockRejectedValue(new Error('No test media'));
    client.resolveAssetDelivery = resolve;
    const view = render(ProfileThumbnail, { client, assetId: 'near', name: 'Thumbnail' });
    expect(resolve).not.toHaveBeenCalled();
    if (!target) throw new Error('Expected an observed thumbnail');
    intersect?.(
        [
            {
                target,
                isIntersecting: true,
                boundingClientRect: target.getBoundingClientRect(),
            } as IntersectionObserverEntry,
        ],
        {} as IntersectionObserver,
    );
    await waitFor(() => expect(resolve).toHaveBeenCalledTimes(1));
    expect(disconnect).not.toHaveBeenCalled();
    view.unmount();
    expect(disconnect).toHaveBeenCalledOnce();
});
