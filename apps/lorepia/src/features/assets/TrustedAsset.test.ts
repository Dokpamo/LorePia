import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type {
    AssetDeliveryDto,
    AssetDeliverySelector,
    LorepiaClient,
} from '../../lib/ipc/contracts';

const tauriMocks = vi.hoisted(() => ({
    convertFileSrc: vi.fn<(filePath: string, protocol?: string) => string>(),
}));

vi.mock('@tauri-apps/api/core', () => tauriMocks);

import TrustedAsset from './TrustedAsset.svelte';

const SHA256 = 'ab'.repeat(32);
const WINDOWS_ASSET_URL = `http://lorepia-asset.localhost/sha256/${SHA256}`;

beforeEach(() => {
    tauriMocks.convertFileSrc.mockReset();
    tauriMocks.convertFileSrc.mockImplementation(
        (filePath: string, protocol = 'asset') =>
            `http://${protocol}.localhost/${encodeURIComponent(filePath)}`,
    );
});

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

function descriptor(
    kind: AssetDeliveryDto['kind'],
    overrides: Partial<AssetDeliveryDto> = {},
): AssetDeliveryDto {
    const mediaTypes = {
        image: 'image/png',
        audio: 'audio/mpeg',
        video: 'video/mp4',
    } as const;
    return {
        asset_id: `asset-${kind}`,
        sha256: SHA256,
        media_type: mediaTypes[kind],
        kind,
        size_bytes: 2048,
        width: kind === 'audio' ? null : 640,
        height: kind === 'audio' ? null : 480,
        duration_ms: kind === 'image' ? null : 3000,
        url: `lorepia-asset://sha256/${SHA256}`,
        ...overrides,
    };
}

function renderAsset(
    value: AssetDeliveryDto,
    selector: AssetDeliverySelector = {
        kind: 'asset_id',
        asset_id: value.asset_id,
    },
) {
    const resolveAssetDelivery = vi.fn().mockResolvedValue(value);
    const client = { resolveAssetDelivery } as unknown as LorepiaClient;
    render(TrustedAsset, {
        client,
        selector,
        alt: '<img src=x onerror=alert(1)>',
        showMetadata: true,
    });
    return resolveAssetDelivery;
}

describe('TrustedAsset', () => {
    it('resolves an opaque image URL and keeps markup in alt text inert', async () => {
        const value = descriptor('image');
        const resolveAssetDelivery = renderAsset(value);

        expect(screen.getByRole('status')).toHaveTextContent('미디어 확인 중');
        const image = await screen.findByRole('img', {
            name: '<img src=x onerror=alert(1)>',
        });
        expect(resolveAssetDelivery).toHaveBeenCalledWith({
            selector: {
                kind: 'asset_id',
                asset_id: 'asset-image',
            },
        });
        expect(tauriMocks.convertFileSrc).toHaveBeenCalledOnce();
        expect(tauriMocks.convertFileSrc).toHaveBeenCalledWith(SHA256, 'lorepia-asset');
        expect(image).toHaveAttribute('src', WINDOWS_ASSET_URL);
        expect(image).toHaveAttribute('referrerpolicy', 'no-referrer');
        expect(document.querySelectorAll('img')).toHaveLength(1);
        expect(document.querySelector('[onerror]')).toBeNull();

        await fireEvent.load(image);
        await waitFor(() => {
            expect(screen.queryByRole('status')).not.toBeInTheDocument();
        });
        expect(screen.getByText(/image\/png · 2.0 KB/)).toBeInTheDocument();
    });

    it.each([
        ['audio', '오디오', 'loadedmetadata'],
        ['video', '비디오', 'loadedmetadata'],
    ] as const)(
        'renders bounded %s media with native controls',
        async (kind, label, readyEvent) => {
            const value = descriptor(kind);
            renderAsset(value);

            const media = await screen.findByLabelText('<img src=x onerror=alert(1)>');
            expect(media.tagName.toLocaleLowerCase()).toBe(kind);
            expect(media).toHaveAttribute('controls');
            expect(media).toHaveAttribute('preload', 'metadata');
            expect(tauriMocks.convertFileSrc).toHaveBeenCalledOnce();
            expect(tauriMocks.convertFileSrc).toHaveBeenCalledWith(SHA256, 'lorepia-asset');
            expect(media).toHaveAttribute('src', WINDOWS_ASSET_URL);
            expect(screen.getByText(`${value.media_type} · 2.0 KB`)).toBeInTheDocument();

            await fireEvent(media, new Event(readyEvent));
            await waitFor(() => {
                expect(screen.queryByRole('status')).not.toBeInTheDocument();
            });
            expect(screen.getByLabelText('<img src=x onerror=alert(1)>')).toBeInTheDocument();
            expect(label).toBe(kind === 'audio' ? '오디오' : '비디오');
        },
    );

    it.each([
        {
            name: 'a mismatched digest URL',
            value: descriptor('image', {
                url: `lorepia-asset://sha256/${'cd'.repeat(32)}`,
            }),
        },
        {
            name: 'a file URL',
            value: descriptor('image', { url: 'file:///Users/synthetic/private.png' }),
        },
        {
            name: 'an active media type',
            value: descriptor('image', { media_type: 'image/svg+xml' }),
        },
        {
            name: 'an oversized image',
            value: descriptor('image', { size_bytes: 16 * 1024 * 1024 + 1 }),
        },
    ])('rejects $name without attaching a media element', async ({ value }) => {
        renderAsset(value);

        expect(await screen.findByRole('alert')).toHaveTextContent(
            '안전하게 표시할 수 없는 미디어입니다.',
        );
        expect(document.querySelector('img, audio, video')).toBeNull();
        expect(document.body.textContent).not.toContain('/Users/');
        expect(document.body.textContent).not.toContain('private.png');
        expect(tauriMocks.convertFileSrc).not.toHaveBeenCalled();
    });

    it.each([
        ['an unexpected protocol', `file://lorepia-asset.localhost/${SHA256}`],
        ['an unexpected host', `http://untrusted.localhost/${SHA256}`],
        ['userinfo', `http://user@lorepia-asset.localhost/${SHA256}`],
        ['a non-default port', `http://lorepia-asset.localhost:8443/${SHA256}`],
        ['an explicit default port', `http://lorepia-asset.localhost:80/${SHA256}`],
        ['a query', `http://lorepia-asset.localhost/${SHA256}?download=1`],
        ['a fragment', `http://lorepia-asset.localhost/${SHA256}#asset`],
        ['an unexpected path', `http://lorepia-asset.localhost/other/${SHA256}`],
    ])('fails closed when the Tauri converter returns $name', async (_name, convertedUrl) => {
        tauriMocks.convertFileSrc.mockReturnValueOnce(convertedUrl);
        renderAsset(descriptor('image'));

        expect(await screen.findByRole('alert')).toHaveTextContent(
            '안전하게 표시할 수 없는 미디어입니다.',
        );
        expect(document.querySelector('img, audio, video')).toBeNull();
    });

    it('fails closed when the Tauri converter throws', async () => {
        tauriMocks.convertFileSrc.mockImplementationOnce(() => {
            throw new Error('/Users/synthetic/private.png');
        });
        renderAsset(descriptor('image'));

        expect(await screen.findByRole('alert')).toHaveTextContent(
            '안전하게 표시할 수 없는 미디어입니다.',
        );
        expect(document.body.textContent).not.toContain('/Users/');
        expect(document.querySelector('img, audio, video')).toBeNull();
    });

    it('rejects a selector/descriptor mismatch and hides transport errors', async () => {
        renderAsset(descriptor('image'), {
            kind: 'asset_id',
            asset_id: 'different-asset',
        });
        expect(await screen.findByRole('alert')).toHaveTextContent(
            '안전하게 표시할 수 없는 미디어입니다.',
        );

        cleanup();
        const client = {
            resolveAssetDelivery: vi
                .fn()
                .mockRejectedValue(new Error('/Users/synthetic/private/asset.png')),
        } as unknown as LorepiaClient;
        render(TrustedAsset, {
            client,
            selector: { kind: 'sha256', sha256: SHA256 },
            alt: '합성 오디오',
        });

        expect(await screen.findByRole('alert')).toHaveTextContent('미디어를 불러오지 못했습니다.');
        expect(document.body.textContent).not.toContain('/Users/');
        expect(document.body.textContent).not.toContain('private');
        expect(document.querySelector('img, audio, video')).toBeNull();
    });

    it('rejects a safe media descriptor when it does not match the consumer kind', async () => {
        const value = descriptor('audio');
        const client = {
            resolveAssetDelivery: vi.fn().mockResolvedValue(value),
        } as unknown as LorepiaClient;
        render(TrustedAsset, {
            client,
            selector: { kind: 'asset_id', asset_id: value.asset_id },
            alt: '캐릭터 이미지',
            expectedKind: 'image',
        });

        expect(await screen.findByRole('alert')).toHaveTextContent(
            '안전하게 표시할 수 없는 미디어입니다.',
        );
        expect(document.querySelector('img, audio, video')).toBeNull();
    });

    it('rejects unexpected descriptor fields instead of retaining a raw path', async () => {
        const unsafeValue = {
            ...descriptor('image'),
            path: '/Users/synthetic/private.png',
        } as unknown as AssetDeliveryDto;
        renderAsset(unsafeValue);

        expect(await screen.findByRole('alert')).toHaveTextContent(
            '안전하게 표시할 수 없는 미디어입니다.',
        );
        expect(document.body.textContent).not.toContain('/Users/');
        expect(document.querySelector('img, audio, video')).toBeNull();
    });

    it('removes a media element after native loading fails', async () => {
        renderAsset(descriptor('video'));
        const video = await screen.findByLabelText('<img src=x onerror=alert(1)>');

        await fireEvent.error(video);

        expect(await screen.findByRole('alert')).toHaveTextContent('미디어를 표시하지 못했습니다.');
        expect(document.querySelector('video')).toBeNull();
    });
});
