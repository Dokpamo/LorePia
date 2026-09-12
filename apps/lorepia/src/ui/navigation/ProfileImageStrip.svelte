<script lang="ts">
    import { onMount, tick } from 'svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { ProfileImage } from './character-profile-types';
    import ProfileThumbnail from './ProfileThumbnail.svelte';
    import { imageStripNavigation } from './image-strip-navigation';
    import './profile-image-strip.css';

    let {
        client,
        images,
        index,
        label,
        onselect,
    }: {
        client: LorepiaClient;
        images: ProfileImage[];
        index: number;
        label: string;
        onselect: (index: number, browsing?: boolean) => void;
    } = $props();
    let strip: HTMLDivElement;
    let navigation: ReturnType<typeof imageStripNavigation> | undefined;
    let previousImages: ProfileImage[];
    $effect(() => {
        const selectedIndex = index;
        const selectedImages = images;
        let current = true;
        void tick().then(() => {
            if (!current || !navigation) return;
            navigation.sync(selectedIndex, selectedImages !== previousImages);
            previousImages = selectedImages;
        });
        return () => {
            current = false;
        };
    });
    onMount(() => {
        previousImages = images;
        navigation = imageStripNavigation(strip, index, (position, browsing) =>
            onselect(position, browsing),
        );
        return () => {
            navigation?.destroy();
            navigation = undefined;
        };
    });
    function key(event: KeyboardEvent, position: number) {
        const next =
            event.key === 'Home'
                ? 0
                : event.key === 'End'
                  ? images.length - 1
                  : event.key === 'ArrowLeft'
                    ? Math.max(0, position - 1)
                    : event.key === 'ArrowRight'
                      ? Math.min(images.length - 1, position + 1)
                      : null;
        if (next === null) return;
        event.preventDefault();
        event.stopPropagation();
        navigation?.choose(next);
        void tick().then(() =>
            strip
                .querySelectorAll<HTMLButtonElement>('button')
                [next]?.focus({ preventScroll: true }),
        );
    }
</script>

<div class="seed-image-strip-fog">
    <div
        bind:this={strip}
        class="seed-image-strip"
        role="toolbar"
        aria-label={label}
        data-image-control
        data-ui-no-swipe
        onwheel={(event) => event.stopPropagation()}
    >
        {#each images as image, i (image.assetId)}
            <button
                type="button"
                class="seed-image-strip-item ui-pressable"
                data-press-feedback="scale"
                aria-label={$tr('navigation.imageThumbnail', { number: i + 1, name: image.title })}
                aria-pressed={i === index}
                tabindex={i === index ? 0 : -1}
                title={image.title}
                onclick={() => navigation?.choose(i)}
                onkeydown={(event) => key(event, i)}
            >
                <span class="ui-press-visual"
                    ><ProfileThumbnail {client} assetId={image.assetId} name={image.title} /></span
                >
            </button>
        {/each}
    </div>
</div>
