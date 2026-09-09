<script lang="ts">
    import { X } from '@lucide/svelte';
    import { getContext, onMount, untrack } from 'svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { ProfileImage } from './character-profile-types';
    import {
        assetPresentationContext,
        type AssetPresentation,
    } from '../workspace/asset-presentation';
    import TrustedAsset from '../../features/assets/TrustedAsset.svelte';
    import { trapFocus } from '../workspace/focus-trap';
    import { imageGestures } from './image-gestures';
    import ProfileImageStrip from './ProfileImageStrip.svelte';
    import './profile-image-viewer.css';

    let {
        client,
        images,
        initialAssetId,
        onclose,
        onimagechange,
    }: {
        client: LorepiaClient;
        images: ProfileImage[];
        initialAssetId?: string;
        onclose: () => void;
        onimagechange?: (assetId: string) => void;
    } = $props();
    const AssetView = getContext<AssetPresentation | undefined>(assetPresentationContext);
    let index = $state(
        untrack(() =>
            Math.max(
                0,
                images.findIndex((image) => image.assetId === initialAssetId),
            ),
        ),
    );
    let dx = $state(0);
    let dy = $state(0);
    let dragging = $state(false);
    let departing = $state<ProfileImage | null>(null);
    let close: HTMLButtonElement;
    onMount(() => {
        index = Math.max(
            0,
            images.findIndex((image) => image.assetId === initialAssetId),
        );
        close.focus({ preventScroll: true });
    });
    function select(position: number) {
        departing =
            Math.abs(position - index) > 1 &&
            !matchMedia('(prefers-reduced-motion: reduce)').matches
                ? (images[index] ?? null)
                : null;
        index = position;
        const selected = images[position];
        if (selected) onimagechange?.(selected.assetId);
    }
    function next() {
        select(Math.min(images.length - 1, index + 1));
    }
    function previous() {
        select(Math.max(0, index - 1));
    }
    function motion(node: HTMLElement, _: unknown, { direction }: { direction: 'in' | 'out' }) {
        const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
        return {
            duration: reduced ? 0 : 220,
            css: (t: number) =>
                direction === 'out'
                    ? `opacity:${String(t)};transform:translateY(${String((1 - t) * node.clientHeight)}px)`
                    : `opacity:${String(t)};transform:scale(${String(0.97 + 0.03 * t)})`,
        };
    }
</script>

{#snippet portrait(image: ProfileImage)}
    {#if AssetView}<AssetView assetId={image.assetId} name={image.title} fit="contain" />{:else}
        <TrustedAsset
            {client}
            selector={/^[a-f0-9]{64}$/.test(image.assetId)
                ? { kind: 'sha256', sha256: image.assetId }
                : { kind: 'asset_id', asset_id: image.assetId }}
            alt={image.title}
            expectedKind="image"
        />
    {/if}
{/snippet}

<div
    class="seed-image-viewer"
    role="dialog"
    aria-modal="true"
    aria-label={$tr('navigation.imageViewer')}
    tabindex="-1"
    transition:motion
    onkeydown={(event) => {
        trapFocus(event);
        if (event.key === 'Escape') {
            event.preventDefault();
            onclose();
        } else if (event.key === 'ArrowLeft') {
            event.preventDefault();
            previous();
        } else if (event.key === 'ArrowRight') {
            event.preventDefault();
            next();
        }
    }}
>
    <div class="seed-image-viewer-backdrop" style:opacity={1 - Math.min(dy / 500, 0.6)}></div>
    <div
        class="seed-image-viewer-surface"
        data-dragging={dragging}
        style:transform={`translateY(${String(dy)}px)`}
        use:imageGestures={{
            next,
            previous,
            dismiss: onclose,
            move: (x, y, active) => {
                dx = x;
                dy = y;
                dragging = active;
            },
        }}
    >
        <header>
            <button
                bind:this={close}
                class="ui-icon-button ui-pressable"
                data-image-control
                aria-label={$tr('uiPreview.back')}
                onclick={onclose}><span class="ui-press-visual"><X /></span></button
            >
            <span class="seed-image-viewer-handle" aria-hidden="true"></span>
        </header>
        <div class="seed-image-viewer-window">
            <div
                class="seed-image-viewer-track"
                data-dragging={dragging}
                data-jumping={departing !== null}
                style:transform={`translateX(calc(${String(-index * 100)}% + ${String(dx)}px))`}
            >
                {#each images as image, i (image.assetId)}
                    <figure aria-hidden={i !== index}>
                        {#if Math.abs(i - index) <= 1}{@render portrait(image)}{/if}
                    </figure>
                {/each}
            </div>
            {#if departing}
                {#key index}<div
                        class="seed-image-viewer-jump"
                        aria-hidden="true"
                        onanimationend={() => (departing = null)}
                    >
                        {@render portrait(departing)}
                    </div>{/key}
            {/if}
        </div>
        <footer>
            <ProfileImageStrip
                {client}
                {images}
                {index}
                label={$tr('navigation.imageNavigation')}
                onselect={select}
            />
            <span class="sr-only" aria-live="polite">{images[index]?.title}</span>
        </footer>
    </div>
</div>
