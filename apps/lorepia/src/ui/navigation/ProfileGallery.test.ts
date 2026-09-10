import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { createPreviewClient } from '../../preview/mock-client';
import { t } from '../../lib/i18n';
import ProfileGallery from './ProfileGallery.svelte';

afterEach(cleanup);

it('opens a folder, opens an uncropped viewer, switches images, and restores the grid before folders', async () => {
    const client = createPreviewClient();
    client.resolveAssetDelivery = vi.fn().mockRejectedValue(new Error('No test media'));
    const onclose = vi.fn();
    render(ProfileGallery, {
        client,
        onclose,
        groups: [
            {
                id: 'person',
                title: 'Ari',
                kind: 'person',
                coverAssetId: 'one',
                images: [
                    { assetId: 'one', title: 'Neutral' },
                    { assetId: 'two', title: 'Smile' },
                ],
            },
        ],
    });
    const folders = screen.getByRole('dialog', { name: t('navigation.profileImages') });
    const folder = within(folders).getByRole('button', {
        name: t('navigation.imageGroupOpen', { name: 'Ari', number: 2 }),
    });
    await fireEvent.click(folder);
    const grid = await screen.findByRole('dialog', { name: 'Ari' });
    const photo = within(grid).getByRole('button', {
        name: t('navigation.imageSelectItem', { name: 'Neutral' }),
    });
    await fireEvent.click(photo);
    const viewer = await screen.findByRole('dialog', { name: t('navigation.imageViewer') });
    expect(grid).toHaveProperty('inert', true);
    const navigation = within(viewer).getByRole('toolbar', {
        name: t('navigation.imageNavigation'),
    });
    const first = within(navigation).getByRole('button', {
        name: t('navigation.imageThumbnail', { number: 1, name: 'Neutral' }),
    });
    const second = within(navigation).getByRole('button', {
        name: t('navigation.imageThumbnail', { number: 2, name: 'Smile' }),
    });
    expect(first).toHaveAttribute('aria-pressed', 'true');
    expect(within(viewer).queryByText('1 / 2')).toBeNull();
    expect(within(viewer).queryByRole('button', { name: t('navigation.nextImage') })).toBeNull();
    await fireEvent.keyDown(viewer, { key: 'ArrowRight' });
    expect(second).toHaveAttribute('aria-pressed', 'true');
    await fireEvent.keyDown(second, { key: 'ArrowLeft' });
    expect(first).toHaveAttribute('aria-pressed', 'true');
    expect(first).toHaveFocus();
    await fireEvent.click(second);
    expect(second).toHaveAttribute('aria-pressed', 'true');
    await fireEvent.keyDown(viewer, { key: 'Escape' });
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([grid]));
    await waitFor(() => expect(photo).toHaveFocus());
    await fireEvent.click(within(grid).getByRole('button', { name: t('uiPreview.back') }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([folders]));
    await waitFor(() => expect(folder).toHaveFocus());
    expect(onclose).not.toHaveBeenCalled();
});
