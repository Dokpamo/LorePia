<script lang="ts">
    import { t, tr } from '../../lib/i18n';
    import { convertFileSrc } from '@tauri-apps/api/core';
    import { SvelteURL } from 'svelte/reactivity';

    import type {
        AssetDeliveryDto,
        AssetDeliverySelector,
        LorepiaClient,
    } from '../../lib/ipc/contracts';

    const SHA256_PATTERN = /^[0-9a-f]{64}$/;
    const MAX_RENDERABLE_ASSET_BYTES = 64 * 1024 * 1024;
    const MAX_RENDERABLE_IMAGE_BYTES = 16 * 1024 * 1024;
    const ALLOWED_MEDIA_TYPES: Readonly<Record<AssetDeliveryDto['kind'], ReadonlySet<string>>> = {
        image: new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/avif']),
        audio: new Set(['audio/mpeg', 'audio/wav', 'audio/ogg']),
        video: new Set(['video/mp4', 'video/webm']),
    };

    interface Props {
        client: LorepiaClient;
        selector: AssetDeliverySelector;
        alt: string;
        showMetadata?: boolean;
        expectedKind?: AssetDeliveryDto['kind'];
    }

    let { client, selector, alt, showMetadata = false, expectedKind }: Props = $props();
    let descriptor = $state<AssetDeliveryDto | null>(null);
    let rendererUrl = $state<string | null>(null);
    let phase = $state<'loading' | 'media_loading' | 'ready' | 'error'>('loading');
    let error = $state<string | null>(null);
    const safeAlt = $derived(alt.slice(0, 512));
    const selectorKind = $derived(selector.kind);
    const selectorValue = $derived(
        selector.kind === 'asset_id' ? selector.asset_id : selector.sha256,
    );

    $effect(() => {
        const activeClient = client;
        const activeSelector: AssetDeliverySelector =
            selectorKind === 'asset_id'
                ? { kind: 'asset_id', asset_id: selectorValue }
                : { kind: 'sha256', sha256: selectorValue };
        const activeExpectedKind = expectedKind;
        let cancelled = false;
        descriptor = null;
        rendererUrl = null;
        error = null;
        phase = 'loading';

        void activeClient
            .resolveAssetDelivery({ selector: activeSelector })
            .then((result) => {
                if (cancelled) return;
                if (
                    !isSafeDescriptor(result, activeSelector) ||
                    (activeExpectedKind !== undefined && result.kind !== activeExpectedKind)
                ) {
                    phase = 'error';
                    error = t('asset.error.unsafe');
                    return;
                }
                const resolvedRendererUrl = rendererAssetUrl(result.sha256);
                if (resolvedRendererUrl === null) {
                    phase = 'error';
                    error = t('asset.error.unsafe');
                    return;
                }
                descriptor = result;
                rendererUrl = resolvedRendererUrl;
                phase = 'media_loading';
            })
            .catch(() => {
                if (cancelled) return;
                phase = 'error';
                error = t('asset.error.load');
            });

        return () => {
            cancelled = true;
        };
    });

    function isSafeOptionalPositiveInteger(value: unknown): value is number | null {
        return (
            value === null ||
            (typeof value === 'number' && Number.isSafeInteger(value) && value > 0)
        );
    }

    function rendererAssetUrl(sha256: string): string | null {
        if (!SHA256_PATTERN.test(sha256)) return null;

        let convertedValue: string;
        try {
            convertedValue = convertFileSrc(sha256, 'lorepia-asset');
        } catch {
            return null;
        }
        if (typeof convertedValue !== 'string') return null;

        let converted: SvelteURL;
        try {
            converted = new SvelteURL(convertedValue);
        } catch {
            return null;
        }

        let expectedOrigin: string;
        if (converted.protocol === 'lorepia-asset:' && converted.hostname === 'localhost') {
            expectedOrigin = 'lorepia-asset://localhost';
        } else if (
            (converted.protocol === 'http:' || converted.protocol === 'https:') &&
            converted.hostname === 'lorepia-asset.localhost'
        ) {
            expectedOrigin = `${converted.protocol}//lorepia-asset.localhost`;
        } else {
            return null;
        }

        const expectedConvertedValue = `${expectedOrigin}/${sha256}`;
        if (
            convertedValue !== expectedConvertedValue ||
            converted.username !== '' ||
            converted.password !== '' ||
            converted.port !== '' ||
            converted.pathname !== `/${sha256}` ||
            converted.search !== '' ||
            converted.hash !== ''
        ) {
            return null;
        }

        converted.pathname = `/sha256/${sha256}`;
        converted.search = '';
        converted.hash = '';
        const finalRendererUrl = converted.toString();
        return finalRendererUrl === `${expectedOrigin}/sha256/${sha256}` ? finalRendererUrl : null;
    }

    function isSafeDescriptor(
        value: unknown,
        expectedSelector: AssetDeliverySelector,
    ): value is AssetDeliveryDto {
        if (typeof value !== 'object' || value === null) return false;
        const candidate = value as Record<string, unknown>;
        const allowedKeys = new Set([
            'asset_id',
            'sha256',
            'media_type',
            'kind',
            'size_bytes',
            'width',
            'height',
            'duration_ms',
            'url',
        ]);
        if (!Object.keys(candidate).every((key) => allowedKeys.has(key))) return false;
        const kind = candidate.kind;
        if (kind !== 'image' && kind !== 'audio' && kind !== 'video') return false;
        if (
            typeof candidate.asset_id !== 'string' ||
            candidate.asset_id.length === 0 ||
            candidate.asset_id.length > 512 ||
            typeof candidate.sha256 !== 'string' ||
            !SHA256_PATTERN.test(candidate.sha256) ||
            typeof candidate.url !== 'string' ||
            candidate.url !== `lorepia-asset://sha256/${candidate.sha256}` ||
            typeof candidate.media_type !== 'string' ||
            !ALLOWED_MEDIA_TYPES[kind].has(candidate.media_type) ||
            !Number.isSafeInteger(candidate.size_bytes) ||
            Number(candidate.size_bytes) <= 0 ||
            Number(candidate.size_bytes) > MAX_RENDERABLE_ASSET_BYTES ||
            (kind === 'image' && Number(candidate.size_bytes) > MAX_RENDERABLE_IMAGE_BYTES) ||
            !isSafeOptionalPositiveInteger(candidate.width) ||
            !isSafeOptionalPositiveInteger(candidate.height) ||
            !isSafeOptionalPositiveInteger(candidate.duration_ms)
        ) {
            return false;
        }
        if (expectedSelector.kind === 'asset_id') {
            return candidate.asset_id === expectedSelector.asset_id;
        }
        return (
            SHA256_PATTERN.test(expectedSelector.sha256) &&
            candidate.sha256 === expectedSelector.sha256
        );
    }

    function mediaReady(): void {
        if (descriptor !== null && rendererUrl !== null) phase = 'ready';
    }

    function mediaFailed(): void {
        descriptor = null;
        rendererUrl = null;
        phase = 'error';
        error = t('asset.error.render');
    }

    function formattedSize(sizeBytes: number): string {
        if (sizeBytes < 1024) return `${String(sizeBytes)} B`;
        return `${(sizeBytes / 1024).toFixed(1)} KB`;
    }
</script>

<div
    class="trusted-asset"
    aria-busy={phase === 'loading' || phase === 'media_loading'}
    data-asset-phase={phase}
>
    {#if descriptor !== null && rendererUrl !== null}
        {#if descriptor.kind === 'image'}
            <img
                src={rendererUrl}
                alt={safeAlt}
                width={descriptor.width ?? undefined}
                height={descriptor.height ?? undefined}
                draggable="false"
                referrerpolicy="no-referrer"
                onload={mediaReady}
                onerror={mediaFailed}
            />
        {:else if descriptor.kind === 'audio'}
            <audio
                src={rendererUrl}
                aria-label={safeAlt || $tr('asset.audio')}
                controls
                preload="metadata"
                onloadedmetadata={mediaReady}
                onerror={mediaFailed}
            ></audio>
        {:else}
            <!-- svelte-ignore a11y_media_has_caption (opaque local assets do not expose a trusted caption track) -->
            <video
                src={rendererUrl}
                aria-label={safeAlt || $tr('asset.video')}
                width={descriptor.width ?? undefined}
                height={descriptor.height ?? undefined}
                controls
                preload="metadata"
                playsinline
                onloadedmetadata={mediaReady}
                onerror={mediaFailed}
            ></video>
        {/if}
    {/if}

    {#if phase === 'loading' || phase === 'media_loading'}
        <span class="asset-status" role="status">
            {phase === 'loading' ? $tr('asset.verifying') : $tr('asset.loading')}
        </span>
    {:else if error !== null}
        <span class="asset-error" role="alert">{error}</span>
    {/if}

    {#if showMetadata && descriptor !== null}
        <small class="asset-metadata">
            {descriptor.media_type} · {formattedSize(descriptor.size_bytes)}
        </small>
    {/if}
</div>

<style>
    .trusted-asset {
        position: relative;
        display: grid;
        width: 100%;
        height: 100%;
        min-height: 1.5rem;
        place-items: center;
        overflow: hidden;
    }

    img,
    video {
        display: block;
        width: 100%;
        height: 100%;
        object-fit: cover;
    }

    audio {
        width: min(100%, 32rem);
    }

    .asset-status,
    .asset-error {
        display: grid;
        position: absolute;
        inset: 0;
        place-items: center;
        padding: 0.25rem;
        color: currentcolor;
        background: color-mix(in srgb, Canvas 82%, transparent);
        font-size: 0.7rem;
        line-height: 1.1;
        text-align: center;
    }

    .asset-error {
        color: #8c2131;
    }

    .asset-metadata {
        position: absolute;
        right: 0.4rem;
        bottom: 0.4rem;
        padding: 0.2rem 0.35rem;
        border-radius: 0.35rem;
        color: white;
        background: rgb(0 0 0 / 68%);
        font-size: 0.7rem;
    }
</style>
