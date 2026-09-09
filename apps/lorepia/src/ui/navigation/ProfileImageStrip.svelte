<script lang="ts">
    import { onMount, tick } from 'svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { ProfileImage } from './character-profile-types';
    import ProfileThumbnail from './ProfileThumbnail.svelte';
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
        onselect: (index: number) => void;
    } = $props();
    let strip: HTMLDivElement;
    let mounted = false;
    let frame = 0;
    function center(animate: boolean) {
        cancelAnimationFrame(frame);
        const selected = strip.querySelector<HTMLElement>('[aria-pressed="true"]');
        if (!selected || !strip.clientWidth) return;
        const from = strip.scrollLeft;
        const to = selected.offsetLeft + selected.offsetWidth / 2 - strip.clientWidth / 2;
        if (!animate || matchMedia('(prefers-reduced-motion: reduce)').matches) {
            strip.scrollLeft = to;
            return;
        }
        const start = performance.now();
        function step(now: number) {
            const progress = Math.min(1, (now - start) / 200);
            strip.scrollLeft = from + (to - from) * (1 - (1 - progress) ** 3);
            if (progress < 1) frame = requestAnimationFrame(step);
        }
        frame = requestAnimationFrame(step);
    }
    $effect(() => {
        void index;
        void images;
        let current = true;
        void tick().then(() => {
            if (current && mounted) center(true);
        });
        return () => {
            current = false;
        };
    });
    onMount(() => {
        mounted = true;
        center(false);
        const observer =
            typeof ResizeObserver === 'undefined'
                ? undefined
                : new ResizeObserver(() => center(false));
        observer?.observe(strip);
        return () => {
            mounted = false;
            observer?.disconnect();
            cancelAnimationFrame(frame);
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
        onselect(next);
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
                aria-label={$tr('navigation.imageThumbnail', { number: i + 1, name: image.title })}
                aria-pressed={i === index}
                tabindex={i === index ? 0 : -1}
                title={image.title}
                onclick={() => onselect(i)}
                onkeydown={(event) => key(event, i)}
            >
                <span class="ui-press-visual"
                    ><ProfileThumbnail {client} assetId={image.assetId} name={image.title} /></span
                >
            </button>
        {/each}
    </div>
</div>
