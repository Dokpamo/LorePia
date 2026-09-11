<script lang="ts">
    import { Image } from '@lucide/svelte';
    import { tick } from 'svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { SampleCharacter } from '../workspace/view-types';
    import CharacterImage from '../workspace/CharacterImage.svelte';
    import type { ProfileImage } from './character-profile-types';
    import { imageGestures } from './image-gestures';

    let {
        character,
        client,
        ondetails,
        images = [],
        onview,
        selectedAssetId = $bindable<string | null>(null),
    }: {
        character: SampleCharacter;
        images?: ProfileImage[];
        selectedAssetId?: string | null;
        onview: (assetId: string, trigger: HTMLButtonElement) => void;
        client: LorepiaClient;
        ondetails: (trigger: HTMLButtonElement) => void;
    } = $props();
    const index = $derived(
        Math.max(
            0,
            images.findIndex((image) => image.assetId === selectedAssetId),
        ),
    );
    let dx = $state(0);
    let dragging = $state(false);
    const active = $derived(images[index]);
    let departing = $state<ProfileImage | null>(null);
    function select(position: number) {
        // Distant jumps crossfade instead of travelling through unloaded slides.
        departing =
            Math.abs(position - index) > 1 &&
            !matchMedia('(prefers-reduced-motion: reduce)').matches
                ? (active ?? null)
                : null;
        selectedAssetId = images[position]?.assetId ?? null;
    }
    function next() {
        select(Math.min(images.length - 1, index + 1));
    }
    function previous() {
        select(Math.max(0, index - 1));
    }
    function imageKey(event: KeyboardEvent, position: number) {
        const target =
            event.key === 'Home'
                ? 0
                : event.key === 'End'
                  ? images.length - 1
                  : event.key === 'ArrowRight'
                    ? Math.min(images.length - 1, position + 1)
                    : event.key === 'ArrowLeft'
                      ? Math.max(0, position - 1)
                      : null;
        if (target === null) return;
        event.preventDefault();
        select(target);
        const track = (event.currentTarget as HTMLElement).parentElement;
        void tick().then(() =>
            track
                ?.querySelectorAll<HTMLButtonElement>('button')
                [target]?.focus({ preventScroll: true }),
        );
    }
    let clipped = $state(false);
    let summaryHeight = $state(0);

    function measureCopy(node: HTMLElement, copy: string) {
        let previousCopy = copy;
        const measure = () => {
            clipped = Array.from(node.querySelectorAll('h1, p')).some(
                (element) => element.scrollHeight > element.clientHeight + 1,
            );
        };
        const observer = new ResizeObserver(measure);
        observer.observe(node);
        return {
            update: (nextCopy: string) => {
                if (previousCopy === nextCopy) return;
                previousCopy = nextCopy;
                queueMicrotask(measure);
            },
            destroy: () => observer.disconnect(),
        };
    }
</script>

{#snippet portrait(image = active)}
    <div class="seed-profile-image">
        {#if image}
            <CharacterImage {client} name={image.title} assetId={image.assetId} />
        {:else}
            <div class="seed-profile-image-empty" aria-hidden="true"><Image /></div>
        {/if}
    </div>
{/snippet}

<section
    class="seed-profile-hero"
    aria-label={$tr('navigation.characterInfo')}
    style:--profile-summary-height={`${String(summaryHeight)}px`}
>
    {#if images.length}
        <div
            class="seed-profile-carousel"
            use:imageGestures={{
                next,
                previous,
                canNext: index < images.length - 1,
                canPrevious: index > 0,
                backAtStart: true,
                move: (x, _y, active) => {
                    dx = x;
                    dragging = active;
                },
            }}
        >
            <div
                class="seed-profile-carousel-track"
                data-dragging={dragging}
                data-jumping={departing !== null}
                style:transform={`translateX(calc(${String(-index * 100)}% + ${String(dx)}px))`}
            >
                {#each images as image, i (image.assetId)}
                    <button
                        type="button"
                        class="seed-profile-slide"
                        tabindex={i === index ? 0 : -1}
                        aria-hidden={i !== index}
                        aria-label={$tr('navigation.imageSelectItem', { name: image.title })}
                        onclick={(event) => onview(image.assetId, event.currentTarget)}
                        onkeydown={(event) => imageKey(event, i)}
                    >
                        {#if Math.abs(i - index) <= 1}{@render portrait(image)}{/if}
                    </button>
                {/each}
            </div>
            {#if departing}
                {#key index}
                    <div
                        class="seed-profile-carousel-jump"
                        aria-hidden="true"
                        onanimationend={() => (departing = null)}
                    >
                        {@render portrait(departing)}
                    </div>
                {/key}
            {/if}
        </div>
    {:else}{@render portrait()}{/if}
    <div
        class="seed-profile-blur"
        aria-hidden="true"
        data-dragging={dragging}
        style:transform={`translateX(${String(index === images.length - 1 ? Math.min(dx, 0) : 0)}px)`}
    >
        {@render portrait()}
    </div>
    <div class="seed-profile-summary" bind:clientHeight={summaryHeight}>
        <div class="seed-profile-copy" use:measureCopy={character.name}>
            <h1 data-scroll-title>{character.name}</h1>
        </div>
        {#if clipped}
            <button
                type="button"
                class="seed-profile-read-more ui-pressable"
                onclick={(event) => ondetails(event.currentTarget)}
                ><span class="ui-press-visual">{$tr('navigation.readTitle')}</span></button
            >
        {/if}
    </div>
</section>
