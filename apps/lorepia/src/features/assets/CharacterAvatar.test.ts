import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import CharacterAvatar from './CharacterAvatar.svelte';
import { t } from '../../lib/i18n';
import type { AssetDeliveryDto } from '../../lib/ipc/contracts';

vi.mock('@tauri-apps/api/core', () => ({
    convertFileSrc: (value: string, protocol: string) => `http://${protocol}.localhost/${value}`,
}));
afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

it('retains the same avatar placeholder through decode retries and terminal failure', async () => {
    const digest = 'ab'.repeat(32);
    const asset: AssetDeliveryDto = {
        asset_id: 'portrait',
        sha256: digest,
        kind: 'image',
        media_type: 'image/png',
        size_bytes: 70,
        width: 1,
        height: 1,
        duration_ms: null,
        url: `lorepia-asset://sha256/${digest}`,
    };
    const client = { resolveAssetDelivery: vi.fn().mockResolvedValue(asset) };
    const view = render(CharacterAvatar, {
        client,
        character: { name: 'Mina', avatar_asset_id: digest },
    });
    const initial = view.container.querySelector('.character-avatar-initial');
    await waitFor(() => expect(view.container.querySelector('img')).not.toBeNull());
    vi.useFakeTimers();
    for (const delay of [1000, 2000]) {
        await fireEvent.error(screen.getByAltText('Mina'));
        expect(view.container.querySelector('.character-avatar-initial')).toBe(initial);
        expect(initial).toHaveTextContent('M');
        expect(view.container.querySelector('.trusted-asset')).toHaveAttribute(
            'data-status-presentation',
            'placeholder',
        );
        await vi.advanceTimersByTimeAsync(delay);
    }
    await fireEvent.error(screen.getByAltText('Mina'));
    expect(view.container.querySelector('img')).toBeNull();
    expect(view.container.querySelector('.character-avatar-initial')).toBe(initial);
    expect(view.container.querySelector('.trusted-asset')).toHaveAttribute(
        'data-asset-phase',
        'error',
    );
    expect(screen.getByRole('alert')).toHaveTextContent(t('asset.error.render'));
    expect(client.resolveAssetDelivery).toHaveBeenCalledOnce();
});
